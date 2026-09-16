# 手机异地访问：自建 WireGuard 中继搭建

本文件对应设计文档 `docs/superpowers/specs/2026-09-15-mobile-control-design.md` 的「方案 2：异地访问」，但**不实现应用层中继**，而是用自建 WireGuard 隧道把手机和电脑放进同一个私网，直接复用已完成的方案 1 局域网网关。

## 为什么这样做

方案 1 的网关在四处主动拒绝公网暴露与隧道（见 `src-tauri/src/mobile/server.rs`）：

| 位置 | 行为 |
| --- | --- |
| `is_allowed_source` | 只认回环、RFC1918 私网、ULA 与链路本地；公网来源 → 403 |
| `host_header_is_allowed` | `Host` 必须是 `localhost` 或私网 IP 字面量；域名 → 403 |
| `has_forwarding_headers` | 六个转发头任一存在 → 400，反代与隧道都过不去 |
| `resolve_bind_ip` | 绑定地址只能是回环或私网 |

因此「把 8787 直接暴露到公网」在代码层面走不通，这正是设计文档禁止该做法的落地依据。

但 WireGuard 是**三层隧道**：它只把 IP 包加密搬运，不添加任何 HTTP 头，也不改变源地址的私网属性。于是把隧道网段选在 `10.8.0.0/24`（`10.0.0.0/8` 属 RFC1918）后，**上述四道闸全部放行，k-Coder 侧不需要任何代码改动**。

隧道网段必须避开 CGNAT 段 `100.64.0.0/10`——那是 Tailscale 与 ZeroTier 使用的网段，不在 `Ipv4Addr::is_private()` 范围内，会被 `is_allowed_source` 拒绝。该前提已由 `mobile::server::tests::wireguard_tunnel_address_passes_every_gate` 与 `cgnat_sources_are_rejected_today` 两条回归测试钉住。

## 拓扑

```text
手机 (10.8.0.3) ──┐
                  ├── WireGuard（UDP 51820）──> VPS（10.8.0.1，公网 IP）
电脑 (10.8.0.2) ──┘
```

手机通过 `https://10.8.0.2:8787/m/` 访问电脑上的 k-Coder 移动网关。VPS 只转发加密的 WireGuard 包，看不到隧道内的 TLS 流量。

## 前置条件

- 一台有公网 IP 的 VPS，最小规格足够（1 核 512 MB）。**不需要域名，不需要购买证书。**
- VPS 能放行 UDP 51820（云厂商安全组 + 系统防火墙都要开）。
- 手机安装官方 WireGuard 客户端。
- 电脑安装 WireGuard 客户端。

## 步骤 1：VPS 安装 WireGuard

Debian / Ubuntu：

```bash
apt update && apt install -y wireguard
```

启用转发并持久化：

```bash
echo 'net.ipv4.ip_forward = 1' > /etc/sysctl.d/99-wireguard.conf
sysctl --system
```

放行端口（如使用 ufw）：

```bash
ufw allow 51820/udp
```

同时到云厂商控制台的安全组里放行 `51820/udp`，只允许 `0.0.0.0/0` 即可（WireGuard 本身不响应未认证包，不会暴露服务）。

## 步骤 2：生成三对密钥

在 VPS 上执行：

```bash
umask 077
wg genkey | tee server.key | wg pubkey > server.pub
wg genkey | tee pc.key     | wg pubkey > pc.pub
wg genkey | tee phone.key  | wg pubkey > phone.pub
```

记录六个值：

```bash
echo "server private: $(cat server.key)"
echo "server public : $(cat server.pub)"
echo "pc private    : $(cat pc.key)"
echo "pc public     : $(cat pc.pub)"
echo "phone private : $(cat phone.key)"
echo "phone public  : $(cat phone.pub)"
```

`phone.key` 与 `pc.key` 属于私钥，配置完对应客户端后应从 VPS 删除（`rm phone.key pc.key`）。它们只用在客户端侧，服务端不需要。

## 步骤 3：服务端配置

`/etc/wireguard/wg0.conf`：

```ini
[Interface]
Address = 10.8.0.1/24
ListenPort = 51820
PrivateKey = <SERVER_PRIVATE_KEY>

[Peer]
# 电脑
PublicKey = <PC_PUBLIC_KEY>
AllowedIPs = 10.8.0.2/32

[Peer]
# 手机
PublicKey = <PHONE_PUBLIC_KEY>
AllowedIPs = 10.8.0.3/32
```

启动并设为开机自启：

```bash
systemctl enable --now wg-quick@wg0
wg show
```

`wg show` 此刻还没有握手记录，等两个客户端连上后才会出现。

## 步骤 4：电脑客户端

新建 `wg0.conf`（Windows 在 WireGuard 客户端里「从文件导入隧道」）：

```ini
[Interface]
PrivateKey = <PC_PRIVATE_KEY>
Address = 10.8.0.2/24

[Peer]
PublicKey = <SERVER_PUBLIC_KEY>
Endpoint = <VPS_PUBLIC_IP>:51820
AllowedIPs = 10.8.0.1/32, 10.8.0.3/32
PersistentKeepalive = 25
```

关键点：`AllowedIPs` **只写隧道内的三个地址，不要写 `0.0.0.0/0`**。写 `0.0.0.0/0` 会把电脑的全部上网流量绕经 VPS，既慢又没必要。

激活隧道后验证：

```bash
ping 10.8.0.1
```

## 步骤 5：手机客户端

在手机 WireGuard 客户端里新建隧道（或用二维码导入）：

```ini
[Interface]
PrivateKey = <PHONE_PRIVATE_KEY>
Address = 10.8.0.3/24

[Peer]
PublicKey = <SERVER_PUBLIC_KEY>
Endpoint = <VPS_PUBLIC_IP>:51820
AllowedIPs = 10.8.0.1/32, 10.8.0.2/32
PersistentKeepalive = 25
```

同样只写隧道地址。激活后手机访问任意网站不受影响，只有去 `10.8.0.x` 的流量走隧道。

## 步骤 6：k-Coder 绑定隧道地址

1. 打开 设置 → 移动设备。
2. **监听地址**填写 `10.8.0.2`。该输入框支持手填，自动探测只能列出默认出口网卡的地址，不会列出隧道地址。
3. 端口保持 `8787`（或自选）。
4. 点击「开启局域网访问」。由于绑定地址不是回环，网关会强制启用 TLS 并生成自签证书。
5. 记录界面上显示的**证书指纹**，手机首次连接时要核对。

### 必须先启动网关，再生成配对二维码

配对 URI 里嵌的是网关**当前实际绑定的地址**（`MobileService::create_pairing` 取 `handle.address.ip()`）。如果先绑定 `192.168.x.x` 生成二维码，二维码里就是局域网地址，手机开着隧道也连不上。

## 步骤 7：手机配对

1. 电脑端点「生成配对二维码」。
2. 手机浏览器打开 `https://10.8.0.2:8787/m/`。
3. 首次会提示证书不受信任——这是自签证书的预期行为，核对指纹后继续。
4. 扫描二维码，或手工把配对链接粘贴进页面。
5. 输入电脑屏幕上显示的 6 位校验码。
6. 电脑端在「待确认设备」里点「允许」。
7. 完成。之后手机可直接在浏览器里查看会话、发消息、处理审批；也可「添加到主屏幕」当 PWA 用。

## 验证清单

| 检查项 | 命令 / 位置 | 期望 |
| --- | --- | --- |
| 服务端握手 | VPS `wg show` | 两个 peer 都有 `latest handshake` |
| 电脑到服务端 | 电脑 `ping 10.8.0.1` | 通 |
| 手机到电脑 | 手机浏览器开 `https://10.8.0.2:8787/health` | 返回含 `protocolVersion=1` 的 JSON |
| 网关绑定 | 设置 → 移动设备 状态徽标 | `https://10.8.0.2:8787` |
| 证书指纹 | 设置页 vs 手机浏览器 | 逐位一致 |
| 会话数据 | 手机端会话列表 | 显示桌面端的真实会话 |

## 已知限制与代价

1. **手机必须常开 WireGuard 客户端**。这是本方案最大的体验损失；应用层中继（方案 2 原设计）才能去掉它。
2. **只能绑定一个地址**。`MobileService::start` 单地址绑定，且 `resolve_bind_ip` 明确拒绝 `0.0.0.0`，所以「局域网直连」与「隧道访问」不能同时提供。建议统一绑 `10.8.0.2`：人在家里时手机走隧道会绕 VPS 一圈，功能正常只是多一次往返。若要两者兼顾，需要改造 `start()` 支持多绑定地址，属于独立改动。
3. **流量绕路**：手机 → VPS → 电脑，延迟高于同网直连。
4. **电脑侧 WireGuard 断开即整个不可用**，且没有降级提示。
5. **绑定地址变化会重新生成证书**，指纹随之改变，手机需要重新核对一次。证书缓存在应用数据目录的 `mobile/tls/` 下，按地址区分。
6. **只有装了 WireGuard 客户端的设备能接入**，无法像应用层中继那样让任何一台手机临时扫码使用。
7. **手机系统的省电策略可能杀掉 WireGuard 后台**，需要把客户端加入电池优化白名单。
8. 未验证项：本方案尚未在真实手机上跑过。仓库既有的原生验收（`scripts/verify-mobile-native.mjs`）证明的是 `192.168.0.220` 这条**同一代码路径**，隧道场景新增的变量只有 WireGuard 能否投递包。

## 排障

| 现象 | 排查方向 |
| --- | --- |
| `wg show` 无握手 | VPS 安全组或系统防火墙未放行 `51820/udp`；`Endpoint` 的 IP 或端口写错 |
| 能握手但打不开 8787 | 电脑上 WireGuard 的 `AllowedIPs` 是否包含 `10.8.0.0/24` 内的对端；k-Coder 是否真的绑在 `10.8.0.2` 而不是 `127.0.0.1` |
| 网关启动报 `invalid_params` | 填的监听地址不是回环或私网地址；`100.64.x.x` 会被拒绝，必须用 `10.x` |
| 手机提示证书错误 | 自签证书的预期行为，核对指纹后继续 |
| 手机端「没有检测到配对链接」 | 二维码是在绑定 `10.8.0.2` 之前生成的；停止网关，按新地址重启后重新生成 |
| 配对提交报 `pairing challenge is unknown or already replaced` | 挑战 10 分钟有效且单次使用，重新生成二维码 |

## 与应用层中继的关系

本方案与设计文档的方案 2 不冲突：它是立刻可用的通道，方案 2 是把「手机装 VPN」这个体验损失去掉的优化。建议先用本方案在真实手机上验证需求是否成立，再决定是否投入应用层中继（那需要新增出站 WSS 客户端、设备非对称密钥、端到端帧加密，并另建中继服务）。

## 本轮代码改动

| 文件 | 改动 |
| --- | --- |
| `src/components/MobileSettingsPage.tsx` | 监听地址由 `<select>` 改为 `<input list>` + `<datalist>`，允许手填隧道地址；新增一条说明文案 |
| `src-tauri/src/mobile/server.rs` | 新增两条回归测试：隧道地址通过全部闸口、CGNAT 段当前被拒绝 |
