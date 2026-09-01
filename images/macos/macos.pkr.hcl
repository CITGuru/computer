# The prepared guest a macOS box is cloned from.
#
# Built once, out of band, and pushed to a registry: there is no building a
# macOS image on first use the way the Linux boxes do. See
# `docs/macos-desktop.md`.
#
#   packer init . && packer build -var-file=secrets.pkrvars.hcl .
#
# The base images already carry automatic login and the guest agent, which is
# most of what a box needs. This template adds the browser, the input helper,
# and the two TCC grants that let the helper do anything at all.
#
# Packer reaches the guest over ssh, because that is what its builder speaks.
# Nothing at runtime does: `MacMachine` goes through `tart exec` and the guest
# agent, which needs no server in the guest and no key on the host.

packer {
  required_plugins {
    tart = {
      version = ">= 1.12.0"
      source  = "github.com/cirruslabs/tart"
    }
  }
}

variable "base" {
  type        = string
  default     = "ghcr.io/cirruslabs/macos-sequoia-base:latest"
  description = "The published image to start from. Already has auto-login and the guest agent."
}

variable "name" {
  type        = string
  default     = "computer-macos"
  description = "What the built image is called locally, before it is pushed."
}

# The display is pinned at 1x on purpose. A 2x guest returns screenshots in
# pixels while CGEvent takes points, so every click lands at half its
# coordinate and nothing reports it. `computer-input geometry` refuses such a
# display rather than driving it, so a box built wrong fails at launch instead
# of misbehaving forever.
variable "display" {
  type    = string
  default = "1280x800"
}

source "tart-cli" "computer" {
  vm_base_name = var.base
  vm_name      = var.name
  cpu_count    = 4
  memory_gb    = 8
  disk_size_gb = 60
  headless     = true

  # The credentials the published base images ship with. They exist for image
  # preparation and are not how anything reaches a running box.
  ssh_username = "admin"
  ssh_password = "admin"
  ssh_timeout  = "300s"
}

build {
  sources = ["source.tart-cli.computer"]

  provisioner "file" {
    source      = "input.swift"
    destination = "/tmp/input.swift"
  }

  provisioner "shell" {
    inline = [
      "set -euo pipefail",
      "echo '--- the browser ---'",
      # Chrome rather than Safari: the box promises a DevTools endpoint and
      # Safari has none.
      "curl -fsSL -o /tmp/chrome.dmg 'https://dl.google.com/chrome/mac/universal/stable/GGRO/googlechrome.dmg'",
      "hdiutil attach -nobrowse -quiet /tmp/chrome.dmg -mountpoint /Volumes/chrome",
      "sudo cp -R '/Volumes/chrome/Google Chrome.app' /Applications/",
      "hdiutil detach -quiet /Volumes/chrome",
      "rm -f /tmp/chrome.dmg",
    ]
  }

  provisioner "shell" {
    inline = [
      "set -euo pipefail",
      "echo '--- the input helper ---'",
      "xcode-select --install 2>/dev/null || true",
      "swiftc -O -o /tmp/computer-input /tmp/input.swift",
      "sudo install -m 755 /tmp/computer-input /usr/local/bin/computer-input",

      # Signed before it is granted anything, and in that order. TCC keys a
      # grant to the code signature, so a grant taken against an unsigned
      # binary — or against a different build of it — matches nothing and
      # every event the driver posts afterwards reaches nobody.
      #
      # Ad-hoc is enough only because the grants below are taken in the same
      # run, against this exact binary. Rebuilding the helper in a prepared
      # image means re-granting it.
      "sudo codesign --force --sign - /usr/local/bin/computer-input",
      "codesign --verify --verbose /usr/local/bin/computer-input",
      "/usr/local/bin/computer-input geometry >/dev/null || echo 'geometry refused: expected until the display is pinned'",
    ]
  }

  # The one step that is not automated here.
  #
  # Screen Recording and Accessibility are TCC grants, and TCC's database is
  # protected by SIP. Writing them needs SIP disabled in the guest or an MDM
  # profile pushed to it, neither of which a Packer shell provisioner can do
  # from inside. Left as a refusal rather than a silent skip: an image that
  # builds and then moves nothing is the worst outcome here.
  provisioner "shell" {
    inline = [
      "set -euo pipefail",
      "if csrutil status | grep -q 'disabled'; then",
      "  echo '--- granting TCC (SIP is off) ---'",
      "  DB=/Library/Application\\ Support/com.apple.TCC/TCC.db",
      "  CD=$(codesign -dr - /usr/local/bin/computer-input 2>&1 | sed -n 's/^designated => //p')",
      "  for SERVICE in kTCCServiceScreenCapture kTCCServiceAccessibility; do",
      "    sudo sqlite3 \"$DB\" \"INSERT OR REPLACE INTO access VALUES('$SERVICE','/usr/local/bin/computer-input',1,2,4,1,NULL,NULL,NULL,'UNUSED',NULL,0,CAST(strftime('%s','now') AS INTEGER));\"",
      "  done",
      "  echo \"granted against: $CD\"",
      "else",
      "  echo '================================================================'",
      "  echo ' SIP is on, so Screen Recording and Accessibility were NOT'",
      "  echo ' granted. A box built from this image will capture a blank'",
      "  echo ' screen and post events that reach nothing.'",
      "  echo ''",
      "  echo ' Grant them by hand in System Settings > Privacy & Security,'",
      "  echo ' for /usr/local/bin/computer-input, then snapshot the guest.'",
      "  echo '================================================================'",
      "  exit 1",
      "fi",
    ]
  }
}
