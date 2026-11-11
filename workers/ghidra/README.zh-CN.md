# Ghidra worker

[English](README.md) · 简体中文

安装 Ghidra 及所需 JDK，将 `WUKONG_GHIDRA_HEADLESS` 设为 `support/analyzeHeadless`（Linux/macOS）或 `support/analyzeHeadless.bat`（Windows）的绝对路径。控制端直接传 argv 启动，不接受 shell 命令模板。Ghidra 12.2 要求 Java 25 JDK，不自动探测其他版本；`JAVA_HOME` 必须指向支持版本，否则 headless 会报告 JDK 不支持及无 TTY 提示错误。Docker 镜像安装校验和固定的 Temurin 25 LTS；本机安装须在启动控制端的环境设置 `JAVA_HOME`。

`GhidraBackend` 导入 SHA256 已核对的本地快照，自动分析并执行 `WukongBridge.java`。Java 桥接调用 function manager、reference manager 和原生反编译器。缺依赖、导入失败、分析/反编译超时、未知地址和格式异常均明确报错；这些操作不执行目标程序。

查询使用**从画像 image base 起算的模块相对偏移**，不使用运行时地址或文件偏移。结果包含类型化地址和原始 Ghidra 地址空间名。函数列表支持有界分页及字面子串搜索；xref 精确查询地址；decompile 接受已发现函数内任意地址。JSON 代码预览最多 65,536 字符，完整内容保留在 `response.json` 旁的 `decompile.c`。

只有样本 SHA256、Ghidra properties/版本、launcher SHA256、bridge SHA256 与分析配置一致时共享项目；查询缓存还包含查询参数。首次导入后使用 `-process target.bin -noanalysis`。每次保留 request、binding、response、stdout、stderr。`worker.lock` 保证独占；控制端崩溃遗留锁时，先确认外部 worker 已停，再删除该锁。项目缺失或缓存被手工修改时由操作者删除/重建；不跨身份使用缓存。

外层超时在 Windows 终止进程树，在 Unix 终止创建的进程组。Ghidra 内部分析超时必须拒绝，不能当作完成；只有分析完成且桥接返回包含预期目标哈希的有效响应后才可复用。

Rust 程序嵌入 bridge，并写入内容寻址脚本目录。开发可用 `script_dir` 指定外部 bridge，其哈希同样参与缓存身份。

原始 API 参考：[Headless analyzer](https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Base/src/main/help/help/topics/HeadlessAnalyzer/HeadlessAnalyzer.htm)、[HeadlessScript](https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Base/src/main/java/ghidra/app/util/headless/HeadlessScript.java)、[DecompInterface](https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Decompiler/src/main/java/ghidra/app/decompiler/DecompInterface.java)。
