# OpenSSH server used by integration tests. Test-only credentials: rooste/rooste.
FROM alpine:3.22

RUN apk add --no-cache openssh \
    && ssh-keygen -A \
    && adduser -D -s /bin/sh rooste \
    && echo "rooste:rooste" | chpasswd \
    && printf 'PasswordAuthentication yes\nAllowTcpForwarding yes\n' >> /etc/ssh/sshd_config

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
