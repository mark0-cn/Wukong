# Windows 原生 worker 组件

[English](README.md) · 简体中文

独立 Cargo 包构建 worker 组件，不链接桌面控制器、WebView 或模型客户端。`native/dbgeng.rs` 与控制器仓库的开发辅助程序共享。组件包版本独立于应用发布版本；交付的 worker 身份还绑定实际源码及二进制内容与运行时协议。

在仓库根目录的 x64 MSVC Native Tools shell 中运行：

```powershell
python -m scripts.worker.build_native --check
python -m scripts.worker.build_native
cargo fmt --manifest-path workers/windows/Cargo.toml -- --check
```

构建入口使用已提交的独立锁文件，并显式设置静态 CRT。默认输出位于 `target/windows-worker-native`。写入 `native-build.json` 前检查普通和延迟加载的 PE 导入，拒绝依赖外部 Visual C++/Universal CRT 库。该检查适用于引导程序自身；离线工具包仍须单独包含第三方工具的全部依赖及原始安装载荷。构建不会执行安装、服务注册或分析目标。

实现依据 [Rust CRT 链接约定](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes)与微软 [PE 格式](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)。Windows CI 对独立 crate 进行格式与代码检查、构建引导程序并验证导入；自动化测试还拒绝动态 CRT、非法库名与格式错误的普通或延迟导入表。

`wukong-worker.exe` 当前提供 `verify-package DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION`，使用共享[离线包约定](../package/README.zh-CN.md)。该只读命令校验独立信任、兼容性与文件精确内容，不安装或激活 worker。其可执行文件与 `wukong-dbgeng.exe` 一起接受静态 CRT 检查。

调试组件为结构化 DbgEng 辅助程序。真实 x64/x86 可信样例验收独立于生产 broker 隔离与离线 VM 安装验收。样例套件还检查重放：使用不同的生产者/验证者 ID，每种架构运行三次新进程与工作目录试验，校验输入及目标的精确哈希，并观测 `0xc0000005` 异常。这证明可信样例重放路径，不代表生产元数据保护或利用影响。托管服务、安装器和更新生命周期仍在实现中；该构建入口不代表这些能力已经可用。参见[跨平台环境约定](../../docs/design/cross-platform-environments.zh-CN.md)和[运行时指南](../runtime/README.zh-CN.md)。
