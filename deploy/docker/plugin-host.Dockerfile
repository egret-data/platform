# syntax=docker/dockerfile:1
# egret's Kestra plugin host (W5b spec §3): Kestra core, the three plugin jars from
# plugin-host/plugins.lock, and the catalog generated from them. One task per JVM; the
# workflow handler supplies the command (`exec java … EgretPluginHost run /work/task.json`).
#
# Build context is plugin-host/, after the Maven build and the jar fetch:
#
#   (cd plugin-host && ./fetch-plugins.sh target/plugins && mvn -q package -DskipTests)
#   docker build -f deploy/docker/plugin-host.Dockerfile -t egret-plugin-host:dev plugin-host
#
# Both base images are pinned by digest (the multi-arch index): Java 25, because Kestra 2.x
# class files are version 69.

# ---- Stage 1: re-verify the jars and generate the catalog ----
FROM eclipse-temurin:25-jdk@sha256:119a3d18f160a3e7655a66034d0f43beee31cd7b3b9142d57a5de29772011de6 AS catalog
WORKDIR /opt/egret-plugin-host
COPY plugins.lock fetch-plugins.sh /src/
COPY target/lib/ lib/
COPY target/egret-plugin-host.jar lib/
COPY target/plugins/ plugins/
# Every jar that ships is checked against plugins.lock here, and a jar it does not name fails the build.
RUN /src/fetch-plugins.sh plugins
# The catalog is a classpath resource (`catalog.json`); the classpath is lib/* and plugins/*,
# which only picks up jars, so it ships as a jar of its own.
RUN java -cp 'lib/*:plugins/*' io.kestra.core.runners.EgretPluginHost describe plugins > /tmp/catalog.json \
 && jar --create --file lib/egret-plugin-catalog.jar -C /tmp catalog.json

# ---- Stage 2: runtime ----
FROM eclipse-temurin:25-jre@sha256:8da0490fa9a3c26867012019565948eef0ee69438f5c75ac28146967bae984b5
COPY --from=catalog /opt/egret-plugin-host /opt/egret-plugin-host
USER 65532:65532
WORKDIR /tmp
# The base image's entrypoint is a shell script; the JVM must be PID 1 to receive SIGTERM itself
# (spec §7.1), so there is no entrypoint and the handler's `exec java …` is the whole command.
ENTRYPOINT []
CMD []
