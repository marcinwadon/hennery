#!/bin/sh
# The collector image starts and is healthy (distribution spec §4.1, §9):
# run with the anonymous volume its `VOLUME` makes, as its own user, the
# collector must find that directory its own (65532, 0700), write it, answer
# `collector healthcheck`, and be reported healthy by Docker's own
# `HEALTHCHECK`. Reading the volume's directory on the host needs `sudo`.
#
# Usage: smoke-image.sh <image>
set -eu

image=$1
name="hennery-smoke-$$"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT

fail() {
    docker logs "$name" >&2 || true
    echo "FAIL: $*" >&2
    exit 1
}

# Docker's checks start one interval after the start: shortened here.
docker run -d --name "$name" --health-interval=2s "$image" >/dev/null

# The healthcheck itself, as the HEALTHCHECK runs it (no shell in the image).
healthy=0
for _ in $(seq 1 30); do
    if docker exec "$name" /usr/local/bin/hennery collector healthcheck; then
        healthy=1
        break
    fi
    sleep 1
done
[ "$healthy" = 1 ] || fail "collector healthcheck never passed"
echo "ok: collector healthcheck passes"

# The database is in the volume, so the collector could write it.
docker exec "$name" /usr/local/bin/hennery admin hosts >/dev/null ||
    fail "the admin socket in the data directory does not answer"
volume=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/var/lib/hennery"}}{{.Source}}{{end}}{{end}}' "$name")
[ -n "$volume" ] || fail "no volume at /var/lib/hennery"
mode=$(sudo stat -c '%u:%g %a' "$volume")
[ "$mode" = "65532:65532 700" ] || fail "the data directory is $mode, not 65532:65532 700"
# The collector warns when it finds its data directory open to others.
if docker logs "$name" 2>&1 | grep -q "other users"; then
    fail "the collector warned about its data directory's permissions"
fi
echo "ok: the data directory is the collector's (65532:65532 700)"

status=
for _ in $(seq 1 30); do
    status=$(docker inspect --format '{{.State.Health.Status}}' "$name")
    [ "$status" = healthy ] && break
    sleep 1
done
[ "$status" = healthy ] || fail "Docker reports the container $status"
echo "ok: Docker reports the container healthy"

# In the image itself: the directory is 65532's and 0700, and its parent is
# still root's (`COPY --from=data … /var/lib/` merges into the existing one).
listing=$(docker export "$name" | tar -tvf - | grep -E ' var/lib/(hennery/)?$' || true)
printf '%s\n' "$listing" | grep -Eq '^drwx------ +65532/65532 .* var/lib/hennery/$' ||
    fail "the image's /var/lib/hennery is not 65532's and 0700: $listing"
printf '%s\n' "$listing" | grep -Eq '^drwxr-xr-x +0/0 .* var/lib/$' ||
    fail "the image's /var/lib is not root's and 0755: $listing"
echo "ok: the image's /var/lib/hennery is 65532's and 0700, /var/lib root's"

user=$(docker inspect --format '{{.Config.User}}' "$image")
[ "$user" = 65532:65532 ] || fail "the image runs as $user"
echo "ok: the image runs as 65532:65532"
