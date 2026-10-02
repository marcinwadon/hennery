# syntax=docker/dockerfile:1@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
# The collector's image (distribution spec §4.1): the released musl binary,
# the same bytes as the archive, on distroless static (CA roots and a
# non-root user, no shell). No Node and no adapters: hosts run next to the
# repositories, never in this image. The `build` workflow stages the binary
# at `dist/linux-<arch>/hennery` (plan 7a). Every image is pinned by its
# multi-arch index digest, the tag beside it for the reader.

# The data directory, owned by the image's user. A volume Docker creates on
# a path the image lacks would be root's, and the collector, running as
# 65532, could not write it. Distroless has no shell to make it with, so a
# stage of the same base's debug variant (busybox) does, 0700. Its parent is
# copied, not the directory: `COPY` of a directory copies what it holds, and
# makes the target with a mode of its own, whatever `--chmod` says.
FROM gcr.io/distroless/static-debian13:debug@sha256:07148a6899406df51906b183f581cf66e5b05fd51aca438bdf1d3998df566961 AS data
RUN ["/busybox/mkdir", "-p", "-m", "0700", "/out/var/lib/hennery"]

FROM gcr.io/distroless/static-debian13:nonroot@sha256:e2e927ec666bae08560abb3c55d0659eceabb657f56b6782ab500a9fc7f555e3
ARG TARGETARCH
COPY --chmod=0755 dist/linux-${TARGETARCH}/hennery /usr/local/bin/hennery
COPY --from=data --chown=65532:65532 /out/var/lib/ /var/lib/
ENV HENNERY_DATA_DIR=/var/lib/hennery HENNERY_LISTEN=0.0.0.0:8080
# Local storage only: SQLite's locking is unreliable on network filesystems.
VOLUME ["/var/lib/hennery"]
USER 65532:65532
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/hennery", "collector", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/hennery"]
CMD ["collector"]
