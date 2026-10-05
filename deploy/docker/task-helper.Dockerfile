# egret's task-pod helper image (W5a, spec §5): the `files-in` init container
# and the `files-out` sidecar of every Kubernetes task pod run this.
#
# Kestra's `PodCreate` uses `busybox` for the same two containers. What egret
# needs from the image is exactly: `sh` (the marker wait loop), `sleep`,
# `tar` (`-x -f -` in, `-c -f - --` out), `cat` (draining an upload whose
# `tar -x` died, so its status arrives), `touch` (the markers), `wc`
# (upload verification) and `tail -F` (streaming main's stderr file) — all busybox applets in Alpine's base image, so the
# image is Alpine with nothing added. It runs as whatever `runAsUser` the pod
# sets (non-root); it needs no user of its own and writes only to the pod's
# two `emptyDir`s.
#
# Build and load into the local kind cluster:
#   docker build -f deploy/docker/task-helper.Dockerfile -t egret-task-helper:dev deploy/docker
#   kind load docker-image egret-task-helper:dev --name egret
FROM alpine:3.20
# Fail the build, not a task, if an applet the runner execs is missing.
RUN for t in cat sh sleep tail tar touch wc; do command -v "$t" >/dev/null || { echo "missing $t"; exit 1; }; done
USER 65532:65532
