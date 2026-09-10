# The same computer in a smaller box.
#
# The X11 contract of `images/desktop/`, with the files a box never opens left
# out: same scripts, same ports, same verbs, about a quarter less to pull. A
# caller points `image_dir()` at this one and gets a smaller box, not a
# different desktop.
#
# Kept in step with `images/desktop/` by `tests/tiny_image.rs`.

# noVNC is static JavaScript that Debian hangs a Node runtime and a Perl
# interpreter off. Taken from the package rather than from upstream so the
# viewer keeps apt's provenance, and taken in a stage the final image discards
# so the box carries the 2 MB it serves instead of the 100 MB it never runs.
FROM debian:bookworm-slim AS viewer

RUN apt-get update && apt-get install -y --no-install-recommends novnc \
  && rm -rf /var/lib/apt/lists/*

FROM debian:bookworm-slim

# **Excluded, never purged.** `dpkg --purge --force-depends` leaves apt's
# dependency graph broken, and the EXTRA_PACKAGES layer below then refuses to
# resolve — a caller who asks for one package gets an unmet-dependency error
# about another. A path-exclude keeps every package installed and its graph
# whole while the bytes never land.
#
# Nothing here is on the driver's path. What goes: software OpenGL, which
# Chromium never calls because it draws through its own SwiftShader; the
# PostScript and PDF delegates ImageMagick would need to convert a document
# rather than capture a screen; a Vulkan validation layer; every Chromium
# locale but one; and the documentation.
RUN printf '%s\n' \
      'path-exclude /usr/share/doc/*' \
      'path-exclude /usr/share/man/*' \
      'path-exclude /usr/share/info/*' \
      'path-exclude /usr/share/locale/*' \
      'path-exclude /usr/share/poppler/*' \
      'path-exclude /usr/share/fonts/type1/urw-base35/*' \
      'path-exclude /usr/lib/*/dri/*' \
      'path-exclude /usr/lib/*/libLLVM-*.so*' \
      'path-exclude /usr/lib/*/libz3.so*' \
      'path-exclude /usr/lib/*/libgs.so*' \
      'path-exclude /usr/lib/gcc/*/*/cc1*' \
      'path-exclude /usr/lib/chromium/libVkLayer_khronos_validation.so' \
      'path-exclude /usr/lib/chromium/locales/*' \
      'path-include /usr/lib/chromium/locales/en-US.pak' \
      > /etc/dpkg/dpkg.cfg.d/01-tiny

# The desktop image's package list without `novnc`, which arrives above as
# files rather than as a dependency tree.
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb \
      x11vnc \
      fluxbox \
      xdotool \
      wmctrl \
      xterm \
      x11-utils \
      x11-xserver-utils \
      imagemagick \
      hsetroot \
      websockify \
      python3 \
      chromium \
      fonts-liberation \
      fonts-dejavu-core \
      dbus-x11 \
      ca-certificates \
      procps \
      socat \
      xclip \
      bash \
  && rm -rf /var/lib/apt/lists/* \
  && mkdir -p /etc/computer/fluxbox \
  # fluxbox shells out to these two; the stubs stop a missing binary from
  # turning into a modal nobody can dismiss on a headless screen.
  && printf '%s\n' '#!/bin/sh' 'exec xsetroot -solid "#111113" "$@"' > /usr/bin/fbsetbg \
  && chmod +x /usr/bin/fbsetbg \
  && ln -sfn /bin/true /usr/bin/xmessage

# Repositories for packages Debian does not carry. One record per line, tab
# separated: name, key URL, deb line. Added before the install below, because a
# package from one of these does not resolve until its archive is known.
ARG EXTRA_SOURCES=""
RUN if [ -n "$EXTRA_SOURCES" ]; then \
      apt-get update \
      && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
      && printf '%s\n' "$EXTRA_SOURCES" | while IFS="$(printf '\t')" read -r name key list; do \
           [ -n "$name" ] || continue; \
           curl -fsSL "$key" | gpg --dearmor -o "/usr/share/keyrings/$name.gpg"; \
           echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/$name.gpg] $list" \
             > "/etc/apt/sources.list.d/$name.list"; \
         done \
      && rm -rf /var/lib/apt/lists/*; \
    fi

# Anything else the caller asked for. The excludes above still apply, so an
# extra package arrives without its manual pages either.
ARG EXTRA_PACKAGES=""
RUN if [ -n "$EXTRA_PACKAGES" ]; then \
      apt-get update \
      && apt-get install -y --no-install-recommends $EXTRA_PACKAGES \
      && rm -rf /var/lib/apt/lists/*; \
    fi

# A launcher per app the box was asked for. After the install above, since an
# entry names a program that install provides.
#
# Through `computer-launch`: a second click on an open app returns to it.
ARG EXTRA_APPS=""
RUN if [ -n "$EXTRA_APPS" ]; then \
      printf '%s\n' "$EXTRA_APPS" | while IFS="$(printf '\t')" read -r name class command; do \
        [ -n "$name" ] || continue; \
        { echo '[Desktop Entry]'; \
          echo 'Type=Application'; \
          echo "Name=$name"; \
          echo "Exec=computer-launch $class $command"; \
          echo "Icon=$(printf '%s' "$command" | cut -d" " -f1 | xargs -r basename)"; \
          echo 'Terminal=false'; \
        } > "/usr/share/applications/computer-app-$name.desktop"; \
      done; \
    fi

ENV DISPLAY=:1
ENV HOME=/home/computer
ENV COMPUTER_SCREEN_WIDTH=1280
ENV COMPUTER_SCREEN_HEIGHT=800
WORKDIR /home/computer

COPY --from=viewer /usr/share/novnc /usr/share/novnc

COPY input-guard.sh /usr/local/bin/xdotool
COPY start.sh     /usr/local/bin/computer-desktop
COPY screen.sh    /usr/local/bin/computer-screen
COPY browser.sh   /usr/local/bin/computer-browser
COPY embed.html /usr/share/novnc/embed.html
COPY fluxbox.apps /etc/computer/fluxbox/apps
COPY fluxbox.menu /etc/computer/fluxbox/menu
COPY fluxbox.init /etc/computer/fluxbox/init
COPY fluxbox.style /etc/computer/fluxbox/style
COPY wallpaper.sh /usr/local/bin/computer-wallpaper
COPY launch.sh /usr/local/bin/computer-launch
COPY tint2rc /etc/computer/tint2rc
COPY terminal.desktop /usr/share/applications/computer-terminal.desktop
COPY browser.desktop /usr/share/applications/computer-browser.desktop

# The terminal's own icon, drawn rather than shipped: xterm's is the X logo,
# which reads as nothing in a dock. ImageMagick is already here for screenshots.
RUN mkdir -p /usr/share/computer \
  && convert -size 128x128 xc:none \
       -fill "#15151b" -stroke "#3d3d4a" -strokewidth 4 \
       -draw "roundrectangle 8,8 120,120 26,26" \
       -stroke "#8fe3a8" -strokewidth 9 -fill none \
       -draw "polyline 42,50 64,68 42,86" \
       -draw "line 74,88 94,88" \
       /usr/share/computer/terminal.png
RUN chmod +x /usr/local/bin/computer-desktop \
             /usr/local/bin/computer-screen \
             /usr/local/bin/computer-browser \
             /usr/local/bin/xdotool \
             /usr/local/bin/computer-wallpaper \
             /usr/local/bin/computer-launch

# The same contract as `images/desktop/`, so the same profile drives it.
LABEL computer.profile="computer-desktop"

EXPOSE 6080 6081 6082 6083 6084 6085 6086 6087 6088 6089 6090 6091 6092 6093 6094 6095
EXPOSE 9222 9223

CMD ["/usr/local/bin/computer-desktop"]
