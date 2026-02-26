# EchoVoice

EchoVoice 是一款基于 Tauri、Rust 和 SvelteKit 构建的简单、低延迟的音频桥接工具。它允许你将音频直接从输入设备（如麦克风或虚拟声卡 VB-Cable）直接传输到输出设备（如扬声器或耳机）。

## 先决条件

- **Node.js**: v18 或更高版本。
- **Rust**: 最新稳定版（通过 rustup 安装）。
- **Windows 构建工具**: 在 Windows 上运行 Tauri 所需的 C++ 构建环境。

## 安装步骤

1. 克隆或下载此代码库。
2. 在项目根目录（`d:\code\rust\EchoVoice`）下打开终端。
3. 安装前端依赖：
   ```bash
   npm install
   ```

## 运行开发模式

要以开发模式启动应用程序（前端支持热重载）：

```bash
npm run tauri dev
```

该命令将：
1. 启动 SvelteKit 的 Vite 开发服务器。
2. 编译并启动 Rust 后端的 Tauri 应用程序。

## 构建生产版本

要创建经过优化、完全独立的独立可执行文件：

```bash
npm run tauri build
```

安装程序和独立的可执行文件将生成在 `src-tauri/target/release/bundle` 目录下。

## 使用说明

1. 打开 EchoVoice 应用程序。
2. 在“Input (Microphone)”（输入/麦克风）下拉菜单中选择一个输入设备。
3. 在“Output (Speakers)”（输出/扬声器）下拉菜单中选择一个输出设备。
4. 点击 **Start Bridge**（开始桥接）。
5. 对着所选的输入设备说话或播放音频，声音应立即通过所选的输出设备播放出来。
6. 点击 **Stop Bridge**（停止桥接）结束音频路由。
