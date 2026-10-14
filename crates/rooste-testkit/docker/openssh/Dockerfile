# OpenSSH server used by integration tests. Test-only credentials: rooste/rooste,
# plus the client keys from crates/rooste-testkit/keys.
FROM alpine:3.24

RUN apk add --no-cache openssh \
    && ssh-keygen -A \
    && adduser -D -s /bin/sh rooste \
    && echo "rooste:rooste" | chpasswd \
    && install -d -o rooste -g rooste -m 700 /home/rooste/.ssh \
    && printf '%s\n' 'PasswordAuthentication yes' 'AllowTcpForwarding yes' \
        '# Tests deliberately fail logins; do not penalise the test client for it.' \
        'PerSourcePenalties no' >> /etc/ssh/sshd_config

COPY --chown=rooste:rooste --chmod=600 authorized_keys /home/rooste/.ssh/authorized_keys

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
