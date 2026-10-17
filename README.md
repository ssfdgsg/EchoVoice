# EchoVoice

EchoVoice 是一款基于 Tauri、Rust 和 SvelteKit 构建的**音效板（Soundboard）+ 麦克风混音台**。
它捕获你的真实麦克风，实时叠加音效，然后把混合后的声音写进 **VB-Audio Virtual Cable（VB-Cable）**，
让游戏 / Discord / OBS 把它当成普通麦克风来收音——就像 [Soundpad](https://www.leppsoft.com/soundpad/)
那样，但完全运行在用户态，不需要自己写驱动。

## 功能特性

### 音效台（对齐 Soundpad 的核心体验）

| 功能 | 说明 |
| --- | --- |
| 多音效同时播放 | 多个音效可以叠加；再次按下同一个快捷键会**重新开始播放**该音效，而不是排队 |
| 每个音效独立音量 | 安静的笑点不必靠喊才能盖过响亮的号角 |
| 循环播放 | 标记为循环后持续播放，直到手动停止 |
| 按住播放（Push-to-Play） | 只有按住快捷键时才播放，松开即淡出停止（即使漏收松开事件，60 秒后也会自动停止） |
| 裁剪 | 跳过开头空白 / 截掉过长结尾，不修改磁盘上的原始文件 |
| 重新触发开关 | 可关闭“再次按下即重新播放”的行为 |
| 分类与收藏 | 按分类筛选、只看收藏、按名称搜索 |
| 拖拽导入 | 把音频文件拖到窗口任意位置即可导入 |
| 麦克风录音 | 一键录制麦克风声音并直接存为新的音效（上限 120 秒） |
| 全局快捷键 | 每个音效一个快捷键，外加三个全局动作：全部停止、静音麦克风、静音音效台 |
| 游戏内可用 | 低层键盘钩子 + `RegisterHotKey` 双通道；如果游戏以管理员运行，EchoVoice 也要以管理员运行 |

### 音乐播放器（游戏里一键放歌）

音乐走混音器里**独立的声道**，不和音效混在一路管理：

- 侧边栏多一个「音乐」页：播放/暂停、上一首、下一首、停止、进度条、独立音量、循环整个列表
- 四个全局快捷键：播放/暂停、下一首、上一首、停止 —— 游戏中聚焦窗口时同样有效
- 一首放完自动切下一首（可关）
- 音效卡片和编辑面板里都能把某个音效标记为「用作音乐」
- 音乐音量和音效台音量分开：为了说话静音音效台时，BGM 不会被一起静掉

### 游戏内快捷键（重点）

| 机制 | 说明 |
| --- | --- |
| 单一派发通道 | 低层键盘钩子（`WH_KEYBOARD_LL`）是**唯一**通道。`RegisterHotKey` 只在钩子装不上时才兜底 |
| 为什么不用 RegisterHotKey | 游戏自己会用 `RegisterHotKey` 占键位，这时我们再注册直接 `ERROR_HOTKEY_ALREADY_REGISTERED`，快捷键就废了；低层钩子照样看得到按键 |
| 按键松开事件 | `RegisterHotKey` 收不到松开，而"按住播放"必须要；低层钩子可以 |
| 修饰键自己追踪 | 修饰键状态由钩子事件维护，每次按键再和 `GetAsyncKeyState` 对账，避免漏收松开导致组合键失灵 |
| 不吞按键 | 钩子只观察不消费，游戏照样收到每一个键 |
| 诊断面板 | 设置页实时显示：监听是否活着、当前通道、是否以管理员运行、已收到按键数、所有已绑定快捷键 |
| 一键提权 | 检测到"游戏以管理员运行"时给出醒目警告和「以管理员身份重启」按钮 |

> ⚠️ **如果游戏里快捷键完全没反应**：Windows 不允许普通进程接收发给"提权进程"的按键。
> 请把 EchoVoice 也以管理员身份运行（诊断面板里有一键重启按钮）。
> 自测方法：切到游戏按一下组合键，看诊断面板的"已收到按键数"有没有涨——
> 涨了说明快捷键是通的，问题在音频链路；不涨说明监听被系统拦掉了。

### 音频与体验优化

- **零首次延迟**：音效在启动桥接时全部预解码并重采样到输出格式，游戏中第一次按快捷键和第十次一样快。
- **淡入淡出**：默认 6 ms 交叉淡变，消除切断音效时的“咔哒”声。
- **软限幅**：超过 -1.4 dBFS 后走 `tanh` 膝形曲线，而不是生硬的硬截波。
- **麦克风闪避（Ducking）**：播放音效时自动压低人声，队友依然听得到你。
- **降噪闸门**：带迟滞的包络跟随闸门，比简单的 RMS 硬切自然得多。
- **三段 EQ / 压缩器 / 变声器**：全部参数持久化保存。
- **实时遥测**：采样率、环形缓冲深度、缓冲延迟、麦克风取数失败次数、丢样、削波数、电平表。
- **设备断开自动重连**：游戏中拔掉耳机（或重装 VB-Cable）会自动关闭桥接，设备回来后按 1s/2s/4s…8s 退避重试并自动重开，不需要切出游戏。
- **托盘常驻**：左键显示窗口，右键可切换桥接 / 全部停止 / 静音麦克风 / 静音音效台 / 退出。
- **深色 / 浅色主题、中英双语、开机自启、最小化到托盘、启动时自动开桥**。

## 工程结构

```
src-tauri/
  crates/echovoice-core/    平台无关的音频核心（混音、DSP、解码、配置）
    src/config.rs           配置模型 + SoundManager（含单元测试）
    src/mixer.rs            多音源实时混音器（含单元测试）
    src/fx.rs               symphonia 解码 + rubato 重采样 + 声道重映射
    src/dsp/                EQ / 压缩器 / 变声器（含单元测试）
    src/wav.rs              WAV 写入 + 麦克风录音器
  src/
    audio/engine.rs         WASAPI 桥接：采集线程 → 环形缓冲 → 渲染线程
    audio/device.rs         设备枚举
    game_hotkey.rs          游戏兼容的低层键盘钩子
    autostart.rs            开机自启（写 HKCU Run 键）
    lib.rs                  Tauri commands / 快捷键分发 / 托盘
src/                        SvelteKit 前端（音效台、路由、设置、教程）
```

把音频核心拆成独立 crate 的好处：`cargo test -p echovoice-core` 可以在**任何机器**上跑
混音器 / DSP / 配置的单元测试，不需要 Windows SDK，也不需要 WASAPI。

## 开发与构建

```bash
# 安装依赖
pnpm install

# 跑音频核心的单元测试（无需 Windows）
cargo test --manifest-path src-tauri/Cargo.toml -p echovoice-core

# 开发模式（前端热重载）
pnpm tauri dev

# 类型检查 / 构建
pnpm check
pnpm build

# Windows 编译检查
cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc

# 打包安装程序（NSIS setup.exe / WiX msi）
pnpm tauri build
```

## 在 Windows 上验证

完整的真机验证流程（含音频链路手测清单、游戏内快捷键 30 秒定位法、提权坑）见
**[docs/VERIFY_ON_WINDOWS.md](docs/VERIFY_ON_WINDOWS.md)**。

最短路径：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml -p echovoice-core   # 97 个测试
pnpm tauri dev                                                       # 跑起来
```

> ⚠️ WSL 里只能 `cargo check`（缺 `link.exe`，构建不出 exe），音频核心测试则在任何平台都能跑。

## 使用说明

1. 安装 [VB-Cable](https://vb-audio.com/Cable/) 并重启电脑。
2. 打开 EchoVoice → **声音路由**：
   - 麦克风输入选你的真实麦克风；
   - 音效台输出选 `CABLE Input (VB-Audio Virtual Cable)`。
3. 在游戏 / Discord / OBS 里把输入设备改成 `CABLE Output (VB-Audio Virtual Cable)`。
4. 点击 **启动音频桥接**，然后就可以点按钮或按快捷键播放音效了。
5. 想把音效台当普通 Soundpad 用（不进游戏语音）时，把输出选成扬声器即可。

> 提示：`CABLE Input` 是声音**进入**虚拟线的入口，`CABLE Output` 是游戏**取出**声音的出口。
> 如果游戏以管理员身份运行，EchoVoice 也必须以管理员身份运行，否则收不到快捷键。
