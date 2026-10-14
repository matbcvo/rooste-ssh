# BusyBox telnetd running a shell without login, for telnet tests only.
FROM alpine:3.24
RUN apk add --no-cache busybox-extras
EXPOSE 23
CMD ["telnetd", "-F", "-l", "/bin/sh", "-p", "23"]
