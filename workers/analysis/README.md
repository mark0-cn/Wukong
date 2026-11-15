# Shared Python analysis

English · [简体中文](README.zh-CN.md)

`wukong_analysis` consumes explicit bytes, parameters or file paths and returns analysis results. It contains classic-BPF decoding/probes, ELF libc candidate comparison, Unicorn simulation and crash-data interpretation. It does not discover workers, manage leases/jobs, launch target processes or persist execution records. Callers admit and confine input paths before invoking it; Linux adapters retain system-library selection and live debugger operations.

Install with `python -m pip install ./workers/analysis`. BPF and crash-location normalization use only the standard library. ELF/core inspection, cyclic-pattern interpretation and emulation use the Linux analysis toolchain (pyelftools, pwntools and Unicorn); importing the package does not load those optional libraries. The Linux image installs the package alongside its worker requirements and analysis toolchain. Windows currently exposes only BPF analysis through a disposable isolated helper and ships the explicit source inventory in `workers/windows/python-components.json`, without Linux worker dependencies.

The Apache-2.0 license is included in `LICENSE`. Installed package sources participate in Linux runtime identity; the native package source identity also includes this directory. Linux tests use `python workers/tests/run.py unit -p 'test_*.py'`; Windows component tests verify the exact copied inventory and execute BPF analysis with only that inventory and the standard library. These tests do not replace actual container or installed-worker acceptance.
