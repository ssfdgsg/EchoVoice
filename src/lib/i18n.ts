export const dict = {
  en: {
    // Navigation
    tab_soundboard: "Soundboard",
    tab_routing: "Audio Routing",
    tab_settings: "Settings",
    tab_guide: "Guide",
    bridge_offline: "Offline",
    bridge_active: "Active",
    btn_start_bridge: "Start Bridge",
    btn_stop_bridge: "Stop Bridge",

    // Soundboard View
    sb_title: "My Soundboard",
    sb_subtitle: "sounds loaded",
    sb_search_placeholder: "Search Sounds...",
    sb_add_sound: "+ Add Sound",
    sb_empty_title: "No Sounds Yet",
    sb_empty_desc: "Click '+ Add Sound' to load some audio files into your soundboard.",
    sb_btn_change_hotkey: "Change Hotkey",
    sb_btn_remove: "Remove",
    sb_bind_hotkey_prompt: "Press any key combo...",

    // Routing View
    rt_title: "Audio Routing",
    rt_subtitle: "Select where the soundboard audio comes from, and where it goes.",
    rt_mic_input: "Microphone Input (Real Voice)",
    rt_sb_output: "Soundboard Output (Virtual Cable)",
    rt_mic_vol: "Microphone Volume",
    rt_fx_vol: "Soundboard Effects Volume",

    // Settings View
    st_title: "Settings",
    st_lang_label: "Language / 语言",
    st_lang_desc: "Choose the display language for the application UI.",
    st_global_controls: "Global Controls",
    st_global_desc: "Instantly stop all audio playing via Soundboard.",
    st_global_placeholder: "Click to bind Global Stop Hotkey",
    st_bg_label: "Custom Background Image",
    st_bg_pick: "Pick Image",
    st_bg_clear: "Clear",
    st_bg_current: "Current:",

    // Guide View
    gd_title: "Setup Guide & Routing",
    gd_subtitle: "Learn how to route EchoVoice audio into Voice Chats (Discord, Steam) and Games.",
    gd_step1_title: "Install VB-Cable (Virtual Audio Cable)",
    gd_step1_desc: "To send soundboard audio directly into your microphone feed without causing echoes for yourself, you need a \"virtual cable\" to connect EchoVoice to your game.",
    gd_step1_btn: "📥 Download VB-Cable (Free)",
    gd_step1_note: "Restart your computer after installation.",
    gd_step2_title: "Configure EchoVoice Routing",
    gd_step2_desc: "Once installed, go to the Audio Routing tab on the left.",
    gd_step2_l1: "Select your real physical microphone (e.g., your headset mic).",
    gd_step2_l2: "Select CABLE Input (VB-Audio Virtual Cable). This forces all soundboard audio to travel through the virtual cable.",
    gd_step2_tip: "💡 Tip: It sounds backwards, but right! 'CABLE Input' is the entrance to the cable (where EchoVoice sends audio), and 'CABLE Output' is the exit (where Discord receives it).",
    gd_step3_title: "Configure Discord / in-Game Voice",
    gd_step3_desc: "Finally, open your game or voice chat software (like Discord, Steam, or VRChat).",
    gd_step3_l1: "Go to their Voice & Audio Settings.",
    gd_step3_l2: "Set your Input Device (Microphone) to CABLE Output (VB-Audio Virtual Cable).",
    gd_step3_success: "You're Done! When you press the Start Bridge button in EchoVoice, your real voice and the soundboard effects will both be mixed and sent to your teammates!",

    // Bottom Player
    pl_playing: "Active",
    pl_ready: "Ready",
    pl_select: "Select a sound to play",
  },
  zh: {
    // Navigation
    tab_soundboard: "音效控制台",
    tab_routing: "声音路由",
    tab_settings: "软件设置",
    tab_guide: "入门教程",
    bridge_offline: "未启动",
    bridge_active: "运行中",
    btn_start_bridge: "启动音频桥接",
    btn_stop_bridge: "停止音频桥接",

    // Soundboard View
    sb_title: "我的音效库",
    sb_subtitle: "个音效已加载",
    sb_search_placeholder: "搜索音效...",
    sb_add_sound: "+ 添加音效",
    sb_empty_title: "这里空空如也",
    sb_empty_desc: "点击右上角的 “+ 添加音效” 按钮来载入你的第一个音频文件吧。",
    sb_btn_change_hotkey: "修改快捷键",
    sb_btn_remove: "移除音效",
    sb_bind_hotkey_prompt: "请按下任意键盘组合...",

    // Routing View
    rt_title: "声音路由设置",
    rt_subtitle: "选择你的麦克风，以及音效要输出到哪个虚拟设备。",
    rt_mic_input: "麦克风输入 (你的真实声音)",
    rt_sb_output: "音效台输出 (虚拟声卡)",
    rt_mic_vol: "麦克风音量",
    rt_fx_vol: "音效台音量",

    // Settings View
    st_title: "系统设置",
    st_lang_label: "语言 / Language",
    st_lang_desc: "修改软件界面的显示语言。",
    st_global_controls: "全局控制快捷键",
    st_global_desc: "一键瞬间停止所有正在播放的音效。",
    st_global_placeholder: "点击绑定“全局停止”快捷键",
    st_bg_label: "自定义背景图片",
    st_bg_pick: "选择图片",
    st_bg_clear: "清除",
    st_bg_current: "当前壁纸:",

    // Guide View
    gd_title: "安装教程与声音路由",
    gd_title_en: "Setup Guide & Routing",
    gd_subtitle: "学习如何将 EchoVoice 的声音完美接入游戏语音 (如 CS:GO) 或聊天软件 (如 Discord, YY)。",
    gd_step1_title: "安装 VB-Cable (免费虚拟声卡)",
    gd_step1_desc: "为了把音效直接灌入队友的耳朵，同时又不让你自己听到回音，我们需要一根“虚拟音频线”把软件和游戏连接起来。",
    gd_step1_btn: "📥 下载 VB-Cable 虚拟声卡 (免费)",
    gd_step1_note: "注意：安装完成后请务必重启电脑！",
    gd_step2_title: "配置 EchoVoice 路由",
    gd_step2_desc: "重启后，点击左侧的【声音路由】标签页：",
    gd_step2_l1: "在 麦克风输入 中，选择你真实的物理麦克风 (比如你的头戴式耳机麦)。",
    gd_step2_l2: "在 音效台输出 中，选择 CABLE Input (VB-Audio Virtual Cable)。这会强制音效跑到虚拟线路里。",
    gd_step2_tip: "💡 提示：为什么是Input？这是因为对于这根虚拟线来说，Input是它的【入口】（软件把声音塞进去），而Output是它的【出口】（游戏把声音吸出来）。",
    gd_step3_title: "配置游戏或聊天软件",
    gd_step3_desc: "最后一步，打开你的游戏或者语音软件（如 YY, Discord 或者 Steam）：",
    gd_step3_l1: "进入它们的 语音/音频设置 界面。",
    gd_step3_l2: "将它们的 麦克风 (输入设备) 修改为 CABLE Output (VB-Audio Virtual Cable)。",
    gd_step3_success: "大功告成！现在只要你在 EchoVoice 中点击左侧的【启动音频桥接】，你说话的声音和你播放的猴叫声，就会一起完美地传给你的队友了！",

    // Bottom Player
    pl_playing: "正在播放",
    pl_ready: "就绪",
    pl_select: "请在上方点击或使用快捷键播放音效",
  }
};
