# Servers for the README screenshots (render_readme in
# crates/rooste-ssh/tests/screenshot.rs): real OpenSSH,
# bash and nginx, so everything the screenshots show is real command output.
# Build from crates/rooste-testkit/docker. Test-only key: crates/rooste-testkit/keys.
FROM debian:stable-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-server nginx-light curl procps \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --no-log-init --shell /bin/bash --groups adm deploy \
    && rm -f /home/deploy/.bashrc /home/deploy/.profile /home/deploy/.bash_logout \
    && mkdir -p /run/sshd /etc/ssh/authorized_keys

# Debian's default colored prompt and `ls` colors (from /etc/skel/.bashrc),
# set system-wide so the home folder holds only the files below.
RUN printf '%s\n' \
        "PS1='\\[\\033[01;32m\\]\\u@\\h\\[\\033[00m\\]:\\[\\033[01;34m\\]\\w\\[\\033[00m\\]\\\$ '" \
        "alias ls='ls --color=auto'" \
        >> /etc/bash.bashrc \
    && printf '%s\n' 'AuthorizedKeysFile /etc/ssh/authorized_keys/%u' 'PrintMotd no' 'PrintLastLog no' \
        > /etc/ssh/sshd_config.d/screenshot.conf \
    && : > /etc/motd && rm -f /etc/update-motd.d/*

COPY --chmod=644 authorized_keys /etc/ssh/authorized_keys/deploy

# A small deployment in deploy's home: nginx serves the current release and
# logs there.
RUN cd /home/deploy \
    && mkdir -p logs releases/2026-10-13 releases/2026-10-16 \
    && echo '<h1>Hello from web-1</h1>' > releases/2026-10-16/index.html \
    && cp releases/2026-10-16/index.html releases/2026-10-13/ \
    && ln -s releases/2026-10-16 current \
    && printf '%s\n' '#!/bin/sh' 'set -e' 'release="releases/$(date +%F)"' 'mkdir -p "$release"' \
        'cp -r build/. "$release/"' 'ln -sfn "$release" current' 'nginx -s reload' > deploy.sh \
    && chmod 755 deploy.sh \
    && printf '%s\n' 'server {' '    listen 80 default_server;' '    root /home/deploy/current;' \
        '    access_log /home/deploy/logs/access.log;' '}' > nginx.conf \
    && tar -czf backup-2026-10-13.tar.gz releases/2026-10-13 \
    && chown -R deploy:deploy /home/deploy \
    && chown root:adm logs && chmod 2775 logs \
    && chmod 755 /home/deploy \
    && ln -sf /home/deploy/nginx.conf /etc/nginx/sites-enabled/default

EXPOSE 22
CMD ["sh", "-c", "nginx && exec /usr/sbin/sshd -D -e"]
