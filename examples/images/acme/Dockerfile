# A custom image, built on the one this crate ships.
#
# The base implements the X11 contract, so an image that only adds to a desktop
# reimplements none of it. `images/ubuntu` is the other way round: a contract
# built from a bare distribution.
#
# The tag is passed in because it follows the bundled image's contents.
# A default only so a build without the argument fails on a missing image
# rather than an empty name.
ARG BASE=computer-desktop:unset
FROM ${BASE}

# Where a tool belongs when it is not worth a `packages()` list per launch.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
         htop \
         jq \
    && rm -rf /var/lib/apt/lists/*

# Something the box has before anything runs in it.
RUN mkdir -p /opt/acme \
    && printf 'built into the image, not written after launch\n' > /opt/acme/README

# **The contract this image implements.** A box driven by another profile is
# refused before it starts. The base is what provides this one.
LABEL computer.profile="computer-desktop"
