# The tag follows the bundled image's contents, so it is passed in.
ARG BASE=computer-desktop:unset
FROM ${BASE}

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
         htop \
         jq \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /opt/acme \
    && printf 'built into the image, not written after launch\n' > /opt/acme/README

LABEL computer.profile="computer-desktop"
