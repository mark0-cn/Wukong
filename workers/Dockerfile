# Build from the project root: docker build -f workers/Dockerfile -t wukong-worker:0.1 .
# Layer order matters for iteration speed:
#   1. big, stable downloads (Ghidra, Temurin JDK) come FIRST so toolchain
#      iterations never invalidate them;
#   2. the Python toolchain uses BuildKit cache mounts so re-runs keep their
#      downloaded wheels;
#   3. COPY workers/runtime comes LAST so script fixes rebuild in seconds.
# The upstream AFL++ image includes binary-only QEMU instrumentation.
ARG AFL_IMAGE=aflplusplus/aflplusplus@sha256:9b168a35bf0d5a270ed96b5017d7012805a8cedb37e40d9fea8f4e8c57200d24
FROM ${AFL_IMAGE} AS toolchain
USER root
ENV DEBIAN_FRONTEND=noninteractive
RUN --mount=type=cache,target=/var/cache/apt \
    --mount=type=cache,target=/var/lib/apt/lists \
    apt-get update && apt-get install -y --no-install-recommends \
    python3 gdb gcc ca-certificates curl unzip python3-pip ruby patchelf \
    && rm -rf /var/lib/apt/lists/* \
    && gem install one_gadget --no-document

# Official Ghidra release and Temurin JDK 25 are fetched ONCE on the host by
# scripts/toolchain/fetch-worker-deps.ps1 into workers/deps/ (checksum-verified there),
# so unchanged attachments can be restored from the master download cache.
# The build verifies their checksums again.
# Set INSTALL_GHIDRA=0 for a smaller runtime-only worker.
ARG INSTALL_GHIDRA=1
ARG GHIDRA_SHA256=2c2cd776db3c1f6a097ba418922378ac3649cadd5e844a6f5f088ae628a7be67
RUN --mount=type=bind,source=workers/deps/ghidra.zip,target=/deps/ghidra.zip,ro \
    if [ "$INSTALL_GHIDRA" = "1" ]; then \
      printf '%s  /deps/ghidra.zip\n' "$GHIDRA_SHA256" | sha256sum --check - \
      && unzip -q /deps/ghidra.zip -d /opt \
      && mv /opt/ghidra_*_PUBLIC /opt/ghidra \
      ; \
    fi
ENV WUKONG_GHIDRA_HEADLESS=/opt/ghidra/support/analyzeHeadless

# Ghidra 11.3+ no longer auto-detects java on PATH; the headless launcher
# requires JAVA_HOME or it tries an interactive JDK prompt and fails.
ARG TEMURIN_SHA256=760f91b22c3efc9bd1b64ff950598231be82928cc807e30691a0e7a2995a62c0
RUN --mount=type=bind,source=workers/deps/temurin25.tar.gz,target=/deps/temurin25.tar.gz,ro \
    printf '%s  /deps/temurin25.tar.gz\n' "$TEMURIN_SHA256" | sha256sum --check - \
    && mkdir -p /opt/jdk-25 && tar -xzf /deps/temurin25.tar.gz -C /opt/jdk-25 --strip-components=1
ENV JAVA_HOME=/opt/jdk-25

# Complete Linux/Python 3.12 analysis and runtime dependency lock. angr 9.2.78
# is the self-contained release (before angr-data); pycparser 2.21 avoids the
# Python 3.12 CLexer breakage. The mixed angr component versions are intentional,
# so the complete set is installed without re-resolving metadata. Hashes also
# cover packages supplied by the AFL base (notably unicorn) by force-reinstalling
# their exact distributions.
COPY workers/requirements.txt /tmp/wukong-image-requirements.txt
ARG PIP_INDEX_URL=https://pypi.org/simple
RUN --mount=type=cache,target=/root/.cache/pip \
    pip3 install --force-reinstall --no-deps --no-build-isolation \
    --require-hashes --break-system-packages -i "$PIP_INDEX_URL" \
    -r /tmp/wukong-image-requirements.txt

ENV PATH="/opt/jdk-25/bin:/AFLplusplus:/usr/local/bin:${PATH}"
# Bake the experiment-UID passwd range and its once-marker at build time: the
# container root filesystem is read-only, so bridge.py cannot append them at
# first use (isolation.py ensure_uid_entries). Numeric UIDs without passwd
# entries break pyvex/getpass inside isolated jobs.
RUN python3 -c "f=open('/etc/passwd','a');[f.write('wukong-%d:x:%d:%d::/nonexistent:/bin/false\n'%(u,u,u)) for u in range(20000,20512)];f.close();open('/etc/wukong-passwd.ready','w').close()"
RUN python3 -c "import angr, claripy, pwn, z3; print('toolchain ok:', angr.__version__)"
RUN useradd --create-home --uid 10001 worker && mkdir -p /state && chown worker:worker /state
# Java derives user.home from the passwd entry (root -> /root), ignoring the
# HOME env var, and Ghidra writes its app config under <user.home>/.config.
# The runtime root filesystem is read-only, so point it at the state volume.
RUN mkdir -p /state/home/.config /state/cache \
    && rm -rf /root/.config \
    && ln -s /state/home/.config /root/.config
# Trusted metadata/supervisors remain root; each target gets a distinct numeric
# UID/GID. Launchers retain CHOWN/SETUID/SETGID/KILL/SYS_PTRACE/DAC_OVERRIDE
# only in trusted supervisors; isolated targets retain no capabilities.
USER root
WORKDIR /state
VOLUME ["/state"]
ENTRYPOINT ["sleep", "infinity"]

# Standalone worker keeps its original runtime layout. Workbench inherits the
# stable toolchain stage instead, so runtime edits cannot invalidate its tools.
FROM toolchain AS worker
COPY workers/analysis /opt/wukong/analysis
RUN pip3 install --break-system-packages --no-deps --no-build-isolation /opt/wukong/analysis
COPY workers/runtime /opt/wukong/runtime
COPY workers/protocol/schema /opt/wukong/protocol/schema
COPY workers/protocol/fixtures /opt/wukong/protocol/fixtures
COPY workers/windows/python/selftest.py /opt/wukong/windows/python/selftest.py
COPY workers/tests /opt/wukong/tests
COPY workers/ghidra /opt/wukong/ghidra
COPY workers/deps/gef.py /opt/gef.py
