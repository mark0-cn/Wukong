# Build from the project root: docker build -f workers/Dockerfile -t re-agent-worker:0.1 .
# Layer order matters for iteration speed:
#   1. big, stable downloads (Ghidra, Temurin JDK) come FIRST so toolchain
#      iterations never invalidate them;
#   2. the Python toolchain uses BuildKit cache mounts so re-runs keep their
#      downloaded wheels;
#   3. COPY workers/runtime comes LAST so script fixes rebuild in seconds.
# The upstream AFL++ image includes binary-only QEMU instrumentation.
ARG AFL_IMAGE=aflplusplus/aflplusplus@sha256:9b168a35bf0d5a270ed96b5017d7012805a8cedb37e40d9fea8f4e8c57200d24
FROM ${AFL_IMAGE}
USER root
ENV DEBIAN_FRONTEND=noninteractive
RUN --mount=type=cache,target=/var/cache/apt \
    --mount=type=cache,target=/var/lib/apt/lists \
    apt-get update && apt-get install -y --no-install-recommends \
    python3 gdb gcc ca-certificates curl unzip python3-pip ruby patchelf \
    && rm -rf /var/lib/apt/lists/* \
    && gem install one_gadget --no-document

# Official Ghidra release and Temurin JDK 25 are fetched ONCE on the host by
# scripts/fetch-worker-deps.ps1 into workers/deps/ (checksum-verified there),
# so builds never re-download and never fail on network flakiness. The build
# verifies the checksums again.
# Set INSTALL_GHIDRA=0 for a smaller runtime-only worker.
ARG INSTALL_GHIDRA=1
ARG GHIDRA_SHA256=2c2cd776db3c1f6a097ba418922378ac3649cadd5e844a6f5f088ae628a7be67
COPY workers/deps/ghidra.zip /tmp/ghidra.zip
RUN if [ "$INSTALL_GHIDRA" = "1" ]; then \
      printf '%s  /tmp/ghidra.zip\n' "$GHIDRA_SHA256" | sha256sum --check - \
      && unzip -q /tmp/ghidra.zip -d /opt \
      && mv /opt/ghidra_*_PUBLIC /opt/ghidra \
      && rm /tmp/ghidra.zip; \
    fi
ENV RE_AGENT_GHIDRA_HEADLESS=/opt/ghidra/support/analyzeHeadless

# Ghidra 11.3+ no longer auto-detects java on PATH; the headless launcher
# requires JAVA_HOME or it tries an interactive JDK prompt and fails.
ARG TEMURIN_SHA256=68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb
COPY workers/deps/temurin25.tar.gz /tmp/temurin25.tar.gz
RUN printf '%s  /tmp/temurin25.tar.gz\n' "$TEMURIN_SHA256" | sha256sum --check - \
    && mkdir -p /opt/jdk-25 && tar -xzf /tmp/temurin25.tar.gz -C /opt/jdk-25 --strip-components=1 \
    && rm /tmp/temurin25.tar.gz
ENV JAVA_HOME=/opt/jdk-25

# Python analysis toolchain used by the typed tools and by agent scripts
# running through job_start: pwntools (pwn plumbing), z3-solver (constraint
# solving), angr (symbolic execution, backing angr.solve) and ropper (gadget
# hunting). angr 9.2.78 is the self-contained release: its libc hook library
# is bundled (angr 10 splits it into angr-data, which arrives broken in the
# AFL base venv) and it pins pycparser<2.22, which Python 3.12's
# "CLexer object is not callable" breakage makes mandatory.
ARG PIP_INDEX_URL=https://mirrors.aliyun.com/pypi/simple/
RUN --mount=type=cache,target=/root/.cache/pip \
    pip3 install --force-reinstall --no-deps --no-cache-dir --break-system-packages \
    -i "$PIP_INDEX_URL" \
    "angr==9.2.78" "pyvex==9.2.131" "archinfo==9.2.102" "cle==9.2.78" \
    "claripy==9.2.101" "ailment==9.2.78" "pycparser==2.21" "bitstring==3.1.9" \
    "z3-solver==4.13.4.0" "ply==3.11" "unicorn==2.1.4" "itanium_demangler==1.1" \
    "protobuf==5.29.4" "capstone==5.0.6" "nampa" "cxxheaderparser" "dpkt" "mulpyplexer" "pypcode" "pefile" "networkx" \
    "cachetools" "sortedcontainers" "pyelftools" "psutil" "requests" "future" "packaging" \
    && pip3 install --no-cache-dir --break-system-packages -i "$PIP_INDEX_URL" \
    pwntools ropper \
    && pip3 install --no-cache-dir --break-system-packages -i "$PIP_INDEX_URL" \
    cffi progressbar2 rich sqlalchemy sympy

ENV PATH="/opt/jdk-25/bin:/AFLplusplus:/usr/local/bin:${PATH}"
# Bake the experiment-UID passwd range and its once-marker at build time: the
# container root filesystem is read-only, so bridge.py cannot append them at
# first use (isolation.py ensure_uid_entries). Numeric UIDs without passwd
# entries break pyvex/getpass inside isolated jobs.
RUN python3 -c "f=open('/etc/passwd','a');[f.write('re-agent-%d:x:%d:%d::/nonexistent:/bin/false\n'%(u,u,u)) for u in range(20000,20512)];f.close();open('/etc/re-agent-passwd.ready','w').close()"
RUN python3 -c "import angr, claripy, pwn, z3; print('toolchain ok:', angr.__version__)"
COPY workers/runtime/requirements.txt /tmp/re-agent-runtime-requirements.txt
RUN pip3 install --no-cache-dir --break-system-packages -i "$PIP_INDEX_URL" -r /tmp/re-agent-runtime-requirements.txt
COPY workers/runtime /opt/re-agent/runtime
COPY workers/ghidra /opt/re-agent/ghidra
COPY workers/deps/gef.py /opt/gef.py
RUN useradd --create-home --uid 10001 worker && mkdir -p /state && chown worker:worker /state
# Java derives user.home from the passwd entry (root -> /root), ignoring the
# HOME env var, and Ghidra writes its app config under <user.home>/.config.
# The runtime root filesystem is read-only, so point it at the state volume.
RUN mkdir -p /state/home/.config /state/cache \
    && rm -rf /root/.config \
    && ln -s /state/home/.config /root/.config
# Trusted metadata/supervisors remain root; each target gets a distinct numeric
# UID/GID. Startup drops all caps except CHOWN/SETUID/SETGID/SYS_PTRACE.
USER root
WORKDIR /state
VOLUME ["/state"]
ENTRYPOINT ["sleep", "infinity"]
