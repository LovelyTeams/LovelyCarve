# Linux 基本信息插件

`plugins/linuxinfo` 是随安装包附带的 Linux 分析插件，插件 ID 为 `org.lovelycarve.linuxinfo`，系统级分析器为 `linuxbaseinfo`，版本以 `plugin.toml` 为准（当前 `2.0.0`，LPP v2）。`plugins/sdk` 是 C ABI / LPP SDK，`plugins/host` 是进程隔离加载器，随 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins) 子模块提供。

## 构建与部署

```text
just build-plugins
just build-plugins --release
just pack-plugins
```

构建把 `linuxinfo.dll`、`liblinuxinfo.so` 或 `liblinuxinfo.dylib` 复制到 `plugin.toml` 旁，并由 `lc-plugin-host --describe` 写出 `schema.json`。桌面安装包和 CLI 包部署 `plugins/linuxinfo/`（清单、库、`schema.json` 与 `locales/`），并把 `lc-plugin-host` 放在主程序旁。`pack-plugins` 另生成可在「设置 → 插件」安装的 `.lcpkg`，带许可文本、来源提交信息和 SHA-256 校验值，输出在 `dist-pack/`。

发现的插件均视为可信代码，隔离进程不提供操作系统权限沙箱。部署和生命周期见 [插件协议](PLUGIN_PROTOCOL.md)。

## 分析范围

宿主对检材的每个卷识别操作系统，`linuxbaseinfo` 在每个识别出的 Linux 系统上各运行一次：在目标检材的「任务中心 → 快速分析」中选择系统和「Linux信息」，或在命令行用 `plugins tasks create --system-id <系统 ID>`。分析在系统视图上进行：系统卷在根部，`/etc/fstab` 中能对应到检材卷的挂载（如独立的 `/home`、`/boot`、LVM 逻辑卷、Btrfs 子卷）出现在挂载点；Docker 容器作为单独的系统识别。

选项 `items` 可选择子项，不选即全部十五项：

| 数据 | 主要来源 |
| --- | --- |
| 系统、时区 | os-release、发行版标志、hostname、timezone / localtime |
| 用户、用户组 | passwd、shadow、group |
| 历史命令、用户痕迹 | Bash / Zsh 历史、GTK XBEL 最近访问 |
| 登录、登录失败、开关机 | wtmp / btmp 的 utmp 记录 |
| 网络 | NetworkManager、OpenWrt UCI、RedHat ifcfg、Hosts 与 DHCP 租约 |
| USB | var/log 中的设备相关文本记录 |
| 服务、启动项 | systemd 单元与 target、init.d |
| 软件 | DPKG status、RPM rpmdb.sqlite |
| 定时任务 | crontab 与用户 cron 文件 |

每种结果是一种带类型的记录（`linux.user`、`linux.login`、`linux.service` 等）：列的类型、标签与时间线角色在 `records.rs` 中声明，英文标签在 `locales/en.toml`。采集到但没有对应列的属性保存在隐藏的 `extra` 字段中，记录详情可见。

记录的来源是系统视图中的文件，宿主换算成卷键与卷内路径，可直接定位原文件；utmp 记录另带字节偏移。登录记录按 PID / 终端配对登出；同名服务的不同来源分别保留。GTK 最近访问保留应用、命令、次数与时间；网络租约保留地址、DNS、网关和租期。配置登记、计划时间与实际运行事件分别解释；无时区的日志时间作为本地时间保存，不伪装为 UTC，也不进入全局时间线。

分析结束时，时区、主机名与用户列表作为画像事实发布（`provides = ["timezone", "hostname", "users"]`），声明 `requires` 的分析器在同一系统上读取它们。

## 读取与验证

`evidence.rs` 提供有大小预算的完整文件读取：缺失路径和非文件返回无内容，超限及短读报错。某个子项失败时记为运行告警，其他子项照常运行；取消立即停止。宿主与结果受 LPP 上限约束。

```text
just fmt
just clippy-plugins
just test-plugins
just build-plugins
just run-example probe_plugin <镜像路径> org.lovelycarve.linuxinfo linuxbaseinfo <结果.json> [--system <系统 ID>]
just run-example probe_plugin_sources <镜像路径> <结果.json>...
```

`probe_plugin` 在临时案件中走完整的任务流程，打印识别出的系统，输出已发布的记录与运行审计；`probe_plugin_sources` 检查结果中每条来源都能定位到镜像中的文件。

可选真实镜像回归：设置 `LOVELYCARVE_TEST_LINUX_IMAGE` 为本机 Linux 镜像路径并运行 `just build-plugins` 后，执行 `just test linuxinfo_analyzes_every_linux_system_of_a_real_image -- --ignored --nocapture`。私有检材和生成结果不提交仓库。
