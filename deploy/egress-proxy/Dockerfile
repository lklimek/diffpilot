# diffpilot egress proxy (v1, S6): Squid as an explicit forward proxy; no SSL-Bump, no CA, no cache.
# Licence and package list in /usr/share/doc/diffpilot (deploy/notices/README.md).
# Build (from the repository root; the named context `workspace` is the repository):
#   docker build --build-context workspace=. -t diffpilot-egress-proxy deploy/egress-proxy
FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db
RUN apt-get update \
 && apt-get install -y --no-install-recommends squid-openssl \
 && rm -rf /var/lib/apt/lists/* \
 && install -d -o proxy -g proxy -m 0750 /run/squid \
 && install -d -m 0755 /usr/share/doc/diffpilot \
 && dpkg-query -W -f='${Package} ${Version} ${source:Package} ${source:Version}\n' \
    > /usr/share/doc/diffpilot/packages.txt
COPY --chmod=0644 --from=workspace LICENSE.md /usr/share/doc/diffpilot/LICENSE.md
COPY --chmod=0755 entrypoint.sh /usr/local/bin/egress-proxy-entrypoint
COPY --chmod=0644 squid.conf /etc/squid/squid.conf
USER proxy
EXPOSE 3128
# Healthy when Squid answers an HTTP request (403 from the manager deny); loopback requests are not access-logged.
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --start-interval=1s CMD ["perl", "-MIO::Socket::INET", "-e", "$s = IO::Socket::INET->new(PeerAddr => q(127.0.0.1:3128), Timeout => 2) or exit 1; print $s qq(GET http://127.0.0.1:3128/squid-internal-mgr/info HTTP/1.0\\r\\n\\r\\n); exit((<$s> // q()) =~ m{^HTTP/1\\.[01] \\d{3}} ? 0 : 1)"]
ENTRYPOINT ["/usr/local/bin/egress-proxy-entrypoint"]
