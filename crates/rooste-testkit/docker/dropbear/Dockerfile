# Dropbear SSH server for interoperability tests. Build from
# crates/rooste-testkit/docker. Test-only credentials: rooste/rooste.
FROM alpine:3.24

RUN apk add --no-cache dropbear \
    && mkdir -p /etc/dropbear \
    && adduser -D -s /bin/sh rooste \
    && echo "rooste:rooste" | chpasswd \
    && install -d -o rooste -g rooste -m 700 /home/rooste/.ssh

COPY --chown=rooste:rooste --chmod=600 authorized_keys /home/rooste/.ssh/authorized_keys

EXPOSE 22
# -F foreground, -E log to stderr, -R generate host keys on demand.
CMD ["dropbear", "-F", "-E", "-R", "-p", "22"]
