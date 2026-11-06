# Build from the project root: docker build -f workers/Dockerfile -t re-agent-worker:0.1 .
# The upstream AFL++ image includes binary-only QEMU instrumentation.
ARG AFL_IMAGE=aflplusplus/aflplusplus@sha256:9b168a35bf0d5a270ed96b5017d7012805a8cedb37e40d9fea8f4e8c57200d24
FROM ${AFL_IMAGE}
USER root
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 gdb gcc ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

# Ghidra 12.2 requires Java 25 (LTS). Ubuntu 24.04 repositories do not carry
# JDK 25, so install the official Temurin 25 LTS build with a pinned checksum.
ARG TEMURIN_URL=https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5%2B8/OpenJDK25U-jdk_x64_linux_hotspot_25.0.5_8.tar.gz
ARG TEMURIN_SHA256=68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb
RUN curl --fail --location --retry 4 "$TEMURIN_URL" -o /tmp/temurin25.tar.gz \
    && printf '%s  /tmp/temurin25.tar.gz\n' "$TEMURIN_SHA256" | sha256sum --check - \
    && mkdir -p /opt/jdk-25 && tar -xzf /tmp/temurin25.tar.gz -C /opt/jdk-25 --strip-components=1 \
    && rm /tmp/temurin25.tar.gz
ENV JAVA_HOME=/opt/jdk-25

# Official Ghidra release, verified against the upstream release checksum.
# Set INSTALL_GHIDRA=0 for a smaller runtime-only worker.
ARG INSTALL_GHIDRA=1
ARG GHIDRA_URL=https://github.com/NationalSecurityAgency/ghidra/releases/download/Ghidra_12.2_build/ghidra_12.2_PUBLIC_20261016.zip
ARG GHIDRA_SHA256=2c2cd776db3c1f6a097ba418922378ac3649cadd5e844a6f5f088ae628a7be67
RUN if [ "$INSTALL_GHIDRA" = "1" ]; then \
      curl --fail --location --retry 4 "$GHIDRA_URL" -o /tmp/ghidra.zip \
      && printf '%s  /tmp/ghidra.zip\n' "$GHIDRA_SHA256" | sha256sum --check - \
      && unzip -q /tmp/ghidra.zip -d /opt \
      && mv /opt/ghidra_*_PUBLIC /opt/ghidra \
      && rm /tmp/ghidra.zip; \
    fi
ENV RE_AGENT_GHIDRA_HEADLESS=/opt/ghidra/support/analyzeHeadless
ENV PATH="/opt/jdk-25/bin:/AFLplusplus:/usr/local/bin:${PATH}"
COPY workers/runtime /opt/re-agent/runtime
COPY workers/ghidra /opt/re-agent/ghidra
RUN useradd --create-home --uid 10001 worker && mkdir -p /state && chown worker:worker /state
# Trusted metadata/supervisors remain root; each target gets a distinct numeric
# UID/GID. Startup drops all caps except CHOWN/SETUID/SETGID/SYS_PTRACE.
USER root
WORKDIR /state
VOLUME ["/state"]
ENTRYPOINT ["sleep", "infinity"]
