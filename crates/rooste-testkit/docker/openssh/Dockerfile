# OpenSSH server used by integration tests. Build from crates/rooste-testkit/docker. Test-only credentials: rooste/rooste,
# plus the client keys from crates/rooste-testkit/keys.
ARG ALPINE=3.24
FROM alpine:${ALPINE}

RUN apk add --no-cache openssh \
    && ssh-keygen -A \
    && adduser -D -s /bin/sh rooste \
    && echo "rooste:rooste" | chpasswd \
    && install -d -o rooste -g rooste -m 700 /home/rooste/.ssh \
    && mkdir -p /etc/ssh/sshd_config.d \
    && printf '%s\n' \
        '# sshd uses the first value it reads; this file is included at the top.' \
        'PasswordAuthentication yes' 'AllowTcpForwarding yes' 'AllowAgentForwarding yes' \
        '# Tests deliberately fail logins; do not penalise the test client for it.' \
        'PerSourcePenalties no' '# Tests connect in parallel.' 'MaxStartups 100' \
        > /etc/ssh/sshd_config.d/rooste-test.conf \
    && (sshd -t 2>/dev/null || sed -i '/PerSourcePenalties/d' /etc/ssh/sshd_config.d/rooste-test.conf) \
    && sshd -t

COPY --chown=rooste:rooste --chmod=600 authorized_keys /home/rooste/.ssh/authorized_keys

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
