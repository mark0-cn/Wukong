# 离线 worker 工具包约定

[English](README.md) · 简体中文

该独立 Rust 库定义供控制器组装和原生程序校验的 Windows worker 签名包格式。格式使用规范 UTF-8 JSON 清单，文件键排序并以一个 LF 结尾，另附 Ed25519 分离签名。校验必须使用独立固定的公钥；随包交付的 `manifest.pub` 不能自行建立对该包的信任。底层签名接口将 PKCS#8 字节返回控制器调用方，不会将私钥序列化到清单或签名中。

`load_controller_signer` 通过排他的原子发布创建稳定控制端身份。Windows 使用当前用户 DPAPI 保护私钥字节，并为目录和文件设置仅用户/SYSTEM 可访问的受保护 ACL。Unix 要求目录及文件属于调用用户，权限分别为 0700/0600。已有不安全权限、链接或损坏密钥直接拒绝，不替换原内容。临时 DPAPI 输出和解密后的文件缓冲区使用后清零；签名对象不能序列化或打印。这保护静态存储密钥不被其他普通用户读取，不防御已失陷的控制端账户或管理员。密钥备份、轮换和桌面登记仍是后续工作；切勿将该目录复制到 VM 载荷或迁移、导出包中。

`seal_package` 先检查兼容性与完整载荷目录，再签名、以不覆盖现有文件的方式写入元数据，最后校验生成的签名目录。控制端工具为 `wukong-package.exe`；worker 运行时不提供签名 RPC，组装器也不会打包该控制端可执行文件。[原生组件指南](../windows/README.zh-CN.md)说明控制端组装步骤。签名工具载荷本身不提供原生服务安装或生产隔离。

公开 VM 登记记录绑定正数代数、安装/环境 ID、控制端签名公钥、具体载荷清单哈希、SSH 管理对端/端口及规范 Ed25519 SSH 公钥，不包含私钥。登记签名使用独立的 `Wukong/Windows-worker-enrollment/v1\0` 域与规范 JSON，限制为 16 KiB；新记录不能替换已有安装信任。首次信任必须由 VM 所有者通过可信控制端交付建立。`VerifiedEnrollment` 要求独立传入公钥，并在发布前绑定已验证载荷；SSH 公钥文本不得包含 authorized-key 选项、命令或换行。重新登记必须保持授权方、安装及环境身份，并高于受保护的最高已接受代数，包括激活失败的请求；相同请求应对账其回执，不得重新激活旧密钥。仅供控制端使用的 `enroll` 实用程序校验实际包文件，并在全新输出目录写入 `enrollment.json`/`enrollment.sig`。这些记录是原生安装器的输入约定；安装会配置专用账号/服务，桌面导入公开连接记录。验收按源码身份记录在 Windows 组件指南中。

清单绑定工具包及源码版本、运行时协议、最低控制器版本、工具链锁文件及哈希、原生/Python/调试器/SSH 入口、每个文件的 SHA256/大小/类型与许可声明。清单 schema 2 要求 WinDbg MCP 与自检入口，拒绝 schema 1，不进行迁移。Python、Debugging Tools、OpenSSH 和 WinDbg MCP 必须提供原始载荷及许可文件。显式 `workers/windows/python-components.json` 清单选择分析源码及仅依赖标准库的 Windows 辅助程序，排除 Linux Worker 模块与依赖。组件声明和有效签名不证明厂商依赖完整或工具可运行；组装器必须检查固定的官方载荷清单，实际离线 VM 安装与自检仍然必需。

限制为每份清单 4 MiB、20,000 个文件、每文件 2 GiB、总载荷 8 GiB。包内路径使用可移植的 ASCII 子集；安装根目录可包含 Unicode。拒绝目录穿越、备用数据流、Windows 设备名、尾部点/空格别名、大小写冲突和文件/目录冲突。完整目录校验还拒绝缺失、额外或变更文件、符号链接以及 Windows 重解析点，包括存在链接的祖先目录。调用方须在校验及复制期间防止不可信进程写入根目录；原生解包仍需受保护 ACL 和安全创建。这些检查自身不提供并发写入隔离。

运行可移植约定检查：

```powershell
cargo fmt --manifest-path workers/package/Cargo.toml -- --check
cargo clippy --locked --manifest-path workers/package/Cargo.toml --all-targets --target-dir target/worker-package-checks -- -D warnings
cargo test --locked --manifest-path workers/package/Cargo.toml --all-targets --target-dir target/worker-package-checks
```

Windows CI 构建静态链接的原生校验程序和控制端工具包实用程序，将 `WUKONG_WORKER_CLI` 与 `WUKONG_PACKAGE_CLI` 指向这两个自行构建的二进制，显式运行默认忽略的 CLI 集成测试。测试封装及校验无执行能力的样例，检查持久化受保护密钥，拒绝载荷内密钥目录，以及不同的固定公钥或被篡改字节；不会执行样例载荷。原生校验命令为 `wukong-worker verify-package DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION`。成功仅涵盖签名、兼容性和文件完整性，不代表安装或生产隔离。参见[原生组件](../windows/README.zh-CN.md)和[环境约定](../../docs/design/cross-platform-environments.zh-CN.md)。
