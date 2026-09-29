use super::File;
use crate::error::{Error, Result};

pub fn inline(dockerfile: &str, files: &[File]) -> Result<String> {
    let mut out = Vec::new();

    for line in dockerfile.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(verb) = words.first() else {
            out.push(line.to_string());
            continue;
        };

        match verb.to_ascii_uppercase().as_str() {
            "COPY" if !words.iter().any(|word| word.starts_with("--from")) => {
                out.extend(copied(&words[1..], files)?);
            }
            "ADD" => {
                return Err(Error::Unsupported {
                    gaps: vec!["ADD in a Dockerfile built at a vendor; use COPY"],
                });
            }
            _ => out.push(line.to_string()),
        }
    }

    Ok(out.join("\n") + "\n")
}

fn copied(words: &[&str], files: &[File]) -> Result<Vec<String>> {
    let mut chmod = None;
    let mut chown = None;
    let mut paths = Vec::new();

    for word in words {
        if let Some(mode) = word.strip_prefix("--chmod=") {
            chmod = Some(mode.to_string());
        } else if let Some(owner) = word.strip_prefix("--chown=") {
            chown = Some(owner.to_string());
        } else if word.starts_with("--") {
            return Err(Error::Unsupported {
                gaps: vec!["a COPY flag this crate cannot inline"],
            });
        } else {
            paths.push(*word);
        }
    }

    let Some((destination, sources)) = paths.split_last() else {
        return Err(Error::denied("a COPY with no destination"));
    };
    if sources.is_empty() {
        return Err(Error::denied("a COPY with no source"));
    }

    let into_directory = destination.ends_with('/') || sources.len() > 1;
    let mut lines = Vec::new();

    for source in sources {
        let source = source.trim_start_matches("./").trim_end_matches('/');
        let chosen: Vec<(&File, String)> = files
            .iter()
            .filter_map(|file| {
                let under = match source {
                    "" | "." => Some(file.path.as_str()),
                    _ if file.path == source => Some(base(&file.path)),
                    _ => file
                        .path
                        .strip_prefix(source)
                        .and_then(|rest| rest.strip_prefix('/')),
                }?;
                Some((file, under.to_string()))
            })
            .collect();

        if chosen.is_empty() {
            return Err(Error::denied(format!(
                "COPY {source}: nothing in the image context by that name"
            )));
        }

        let whole_file = chosen.len() == 1 && chosen[0].0.path == source;
        for (file, under) in chosen {
            let target = match (whole_file && !into_directory, destination.ends_with('/')) {
                (true, _) => (*destination).to_string(),
                (false, true) => format!("{destination}{under}"),
                (false, false) => format!("{destination}/{under}"),
            };
            lines.push(written(file, &target, chmod.as_deref(), chown.as_deref()));
        }
    }

    Ok(lines)
}

fn written(file: &File, target: &str, chmod: Option<&str>, chown: Option<&str>) -> String {
    let quoted = quote(target);
    let mode = chmod.map_or_else(|| format!("{:o}", file.mode), str::to_string);
    let mut line = format!(
        "RUN mkdir -p \"$(dirname {quoted})\" && echo {} | base64 -d > {quoted} && chmod {mode} {quoted}",
        crate::cdp::base64_encode(&file.bytes),
    );
    if let Some(owner) = chown {
        line.push_str(&format!(" && chown {} {quoted}", quote(owner)));
    }
    line
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<File> {
        vec![
            File::new("start.sh", b"#!/bin/sh\n".to_vec()),
            File::new("fluxbox.apps", b"apps".to_vec()),
            File::new("app/one.txt", b"1".to_vec()),
            File::new("app/deep/two.txt", b"2".to_vec()),
        ]
    }

    fn decoded(line: &str) -> (String, Vec<u8>) {
        let encoded = line
            .split("echo ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .expect("an encoded file");
        let target = line
            .split("base64 -d > ")
            .nth(1)
            .and_then(|rest| rest.split(" &&").next())
            .expect("a target");
        (
            target.trim_matches('\'').to_string(),
            crate::cdp::base64_decode(encoded).expect("base64"),
        )
    }

    #[test]
    fn test_a_copied_file_is_written_by_the_build_itself() {
        let dockerfile = inline(
            "FROM debian\nCOPY start.sh     /usr/local/bin/computer-desktop\nRUN true\n",
            &files(),
        )
        .expect("inlined");

        let lines: Vec<&str> = dockerfile.lines().collect();
        assert_eq!(lines[0], "FROM debian");
        assert_eq!(lines[2], "RUN true", "every other line is left as it was");
        assert_eq!(
            decoded(lines[1]),
            (
                "/usr/local/bin/computer-desktop".to_string(),
                b"#!/bin/sh\n".to_vec()
            ),
            "daytona builds with no context, so the bytes ride in the Dockerfile"
        );
    }

    #[test]
    fn test_a_directory_destination_keeps_each_file_name() {
        let dockerfile =
            inline("COPY start.sh fluxbox.apps /etc/computer/\n", &files()).expect("inlined");

        let targets: Vec<String> = dockerfile.lines().map(|line| decoded(line).0).collect();
        assert_eq!(
            targets,
            ["/etc/computer/start.sh", "/etc/computer/fluxbox.apps"]
        );
    }

    #[test]
    fn test_a_copied_directory_keeps_its_layout() {
        let dockerfile = inline("COPY app /opt/app\n", &files()).expect("inlined");

        let targets: Vec<String> = dockerfile.lines().map(|line| decoded(line).0).collect();
        assert_eq!(targets, ["/opt/app/one.txt", "/opt/app/deep/two.txt"]);
    }

    #[test]
    fn test_a_stage_copy_is_left_to_the_builder() {
        let line = "COPY --from=pointer /src/computer-pointer /usr/local/bin/computer-pointer";

        assert_eq!(inline(line, &files()).expect("inlined").trim_end(), line);
    }

    #[test]
    fn test_a_mode_and_owner_given_to_copy_are_kept() {
        let dockerfile =
            inline("COPY --chmod=755 --chown=1000:1000 start.sh /s\n", &files()).expect("inlined");

        assert!(dockerfile.contains("chmod 755 '/s'"));
        assert!(dockerfile.contains("chown '1000:1000' '/s'"));
    }

    #[test]
    fn test_a_source_that_is_not_there_is_refused() {
        assert!(inline("COPY missing.sh /x\n", &files()).is_err());
        assert!(
            inline("ADD start.sh /x\n", &files()).is_err(),
            "ADD can fetch and unpack, which a written file cannot stand in for"
        );
    }

    #[test]
    fn test_a_quote_cannot_leave_its_word() {
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
