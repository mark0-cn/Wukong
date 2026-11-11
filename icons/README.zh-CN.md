<a id="wukong-icons"></a>

# Wukong 图标

[English](README.md) · 简体中文

唯一设计源为 `icon.svg`。深墨绿底色延续工作台的视觉环境；金色紧箍的内卷曲线、猴王面具和橙色眼缝分别表达悟空、辨识和洞察。
轮廓使用对称几何与大块留白，小尺寸优先识别金箍和面部轮廓，避免依赖文字、细线或复杂纹理。

侧边栏和欢迎页直接使用同一个 SVG，Windows ICO、macOS ICNS 和各尺寸 PNG 由锁定版本的 Tauri CLI 生成：

```powershell
./scripts/dev/generate-icons.ps1
```

需要先安装根目录 npm 依赖。生成中间文件位于忽略的 `target/generated-icons`；脚本只更新桌面所需的图标，不把移动端图标加入仓库。
设计变更后检查 32px、64px 及应用内效果，并重新生成全部桌面资源。
