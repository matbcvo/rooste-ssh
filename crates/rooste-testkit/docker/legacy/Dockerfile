# An OpenSSH server that only offers outdated algorithms, like an old
# network device. rooste-ssh must refuse it by default and connect when
# legacy algorithms are enabled. Build from crates/rooste-testkit/docker.
FROM alpine:3.24

RUN apk add --no-cache openssh \
    && ssh-keygen -A \
    && adduser -D -s /bin/sh rooste \
    && echo "rooste:rooste" | chpasswd \
    && install -d -o rooste -g rooste -m 700 /home/rooste/.ssh \
    && mkdir -p /etc/ssh/sshd_config.d \
    && printf '%s\n' \
        'PasswordAuthentication yes' 'PerSourcePenalties no' 'MaxStartups 100' \
        'KexAlgorithms diffie-hellman-group14-sha1' \
        'HostKey /etc/ssh/ssh_host_rsa_key' 'HostKeyAlgorithms ssh-rsa' \
        'PubkeyAcceptedAlgorithms +ssh-rsa' \
        'Ciphers aes128-cbc' 'MACs hmac-sha1' \
        > /etc/ssh/sshd_config.d/rooste-test.conf

COPY --chown=rooste:rooste --chmod=600 authorized_keys /home/rooste/.ssh/authorized_keys

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
