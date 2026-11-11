# 离线 worker 工具包约定

[English](README.md) · 简体中文

该独立 Rust 库定义供控制器组装和原生程序校验的 Windows worker 签名包格式。格式使用规范 UTF-8 JSON 清单，文件键排序并以一个 LF 结尾，另附 Ed25519 分离签名。校验必须使用独立固定的公钥；随包交付的 `manifest.pub` 不能自行建立对该包的信任。签名接口将 PKCS#8 字节返回控制器调用方，不会将私钥序列化到清单或签名中。受保护的控制器密钥存储和完整工具包组装器属于后续实现。

清单绑定工具包及源码版本、运行时协议、最低控制器版本、工具链锁文件及哈希、原生/Python/调试器/SSH 入口、每个文件的 SHA256/大小/类型与许可声明。Python、Debugging Tools、OpenSSH 和 worker Python 依赖都必须提供原始安装载荷及许可文件。组件声明和有效签名不证明厂商依赖完整或工具可运行；组装器必须检查固定的官方载荷清单，实际离线 VM 安装与自检仍然必需。

限制为每份清单 4 MiB、20,000 个文件、每文件 2 GiB、总载荷 8 GiB。包内路径使用可移植的 ASCII 子集；安装根目录可包含 Unicode。拒绝目录穿越、备用数据流、Windows 设备名、尾部点/空格别名、大小写冲突和文件/目录冲突。完整目录校验还拒绝缺失、额外或变更文件、符号链接以及 Windows 重解析点，包括存在链接的祖先目录。调用方须在校验及复制期间防止不可信进程写入根目录；原生解包仍需受保护 ACL 和安全创建。这些检查自身不提供并发写入隔离。

运行可移植约定检查：

```powershell
cargo fmt --manifest-path workers/package/Cargo.toml -- --check
cargo clippy --locked --manifest-path workers/package/Cargo.toml --all-targets --target-dir target/worker-package-checks -- -D warnings
cargo test --locked --manifest-path workers/package/Cargo.toml --all-targets --target-dir target/worker-package-checks
```

Windows CI 还构建静态链接的原生校验程序，将 `WUKONG_WORKER_CLI` 指向该自行构建的二进制，显式运行默认忽略的 CLI 集成测试。测试验证无执行能力的签名样例，并拒绝不同的固定公钥与被篡改字节；不会执行样例载荷。原生命令为 `wukong-worker verify-package DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION`。成功仅涵盖签名、兼容性和文件完整性，不代表安装或生产隔离。参见[原生组件](../windows/README.zh-CN.md)和[环境约定](../../docs/design/cross-platform-environments.zh-CN.md)。
