import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  Link2,
  LoaderCircle,
  Lock,
  Play,
  QrCode,
  RefreshCw,
  ShieldCheck,
  Smartphone,
  Square,
  Trash2,
  Wifi,
  XCircle,
} from "lucide-react";
import QRCode from "qrcode";
import {
  approveMobilePairing,
  createMobilePairing,
  denyMobilePairing,
  errorMessage,
  mobileStatus,
  revokeMobileDevice,
  setMobileCapabilities,
  startMobileGateway,
  stopMobileGateway,
} from "../api/runtime";
import type {
  MobileCapability,
  MobileDeviceView,
  MobilePairingView,
  MobileStatus,
} from "../types/runtime";
import { useToast } from "./Toast";
import "./MobileSettingsPage.css";

const CAPABILITY_LABELS: Record<MobileCapability, string> = {
  chat: "查看会话与发送消息",
  approval: "处理审批请求",
  interrupt: "停止运行中的 Turn",
  fileRead: "读取工作区文件",
  shell: "执行 Shell 命令",
  settings: "修改应用设置",
  plugins: "启停插件",
  secrets: "读取密钥",
};

/** 本期已经实现的能力。其余能力即使显示出来也只会在手机上返回「未开放」。 */
const IMPLEMENTED_CAPABILITIES: MobileCapability[] = ["chat", "approval", "interrupt"];

function formatTime(timestampMs: number): string {
  if (!timestampMs) return "—";
  return new Date(timestampMs).toLocaleString();
}

function deviceStatusLabel(device: MobileDeviceView): string {
  if (device.revoked) return "已撤销";
  return "已授权";
}

export function MobileSettingsPage() {
  const toast = useToast();
  const [status, setStatus] = useState<MobileStatus | null>(null);
  const [pairing, setPairing] = useState<MobilePairingView | null>(null);
  const [qrDataUrl, setQrDataUrl] = useState("");
  const [bindAddress, setBindAddress] = useState("");
  const [port, setPort] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const pollingRef = useRef<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await mobileStatus();
      setStatus(next);
      setPairing(next.pairing);
      setBindAddress((current) => current || next.preferredBindAddress || "");
      setPort((current) => current || String(next.preferredPort));
      setError("");
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 待确认配对需要持续轮询：手机提交后必须由电脑这边确认。
  useEffect(() => {
    if (!status?.running) return;
    pollingRef.current = window.setInterval(() => {
      void refresh();
    }, 3000);
    return () => {
      if (pollingRef.current !== null) {
        window.clearInterval(pollingRef.current);
        pollingRef.current = null;
      }
    };
  }, [status?.running, refresh]);

  useEffect(() => {
    if (!pairing?.uri) {
      setQrDataUrl("");
      return;
    }
    let cancelled = false;
    QRCode.toDataURL(pairing.uri, { margin: 1, width: 240 })
      .then((value) => {
        if (!cancelled) setQrDataUrl(value);
      })
      .catch(() => {
        if (!cancelled) setQrDataUrl("");
      });
    return () => {
      cancelled = true;
    };
  }, [pairing?.uri]);

  const lanAddresses = useMemo(() => status?.lanAddresses ?? [], [status?.lanAddresses]);
  const devices = status?.devices ?? [];
  // 待确认请求取自 status 而不是 pairing：手机一提交，挑战就被消费、`pairing` 立刻变回
  // null，挂在它下面的待确认列表会在提交成功的那一刻消失，桌面端再也看不到「允许」按钮。
  const pending = status?.pendingPairings ?? [];
  const granted = useMemo(
    () => new Set(status?.capabilities ?? []),
    [status?.capabilities],
  );

  async function run(label: string, action: () => Promise<void>) {
    setBusy(label);
    try {
      await action();
      setError("");
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      toast.error(message);
    } finally {
      setBusy(null);
    }
  }

  const start = () =>
    run("start", async () => {
      const parsedPort = Number.parseInt(port, 10);
      if (!Number.isInteger(parsedPort) || parsedPort < 1 || parsedPort > 65535) {
        throw new Error("端口必须是 1..65535 之间的整数");
      }
      if (!bindAddress.trim()) {
        throw new Error("局域网访问必须选择一个本机私网地址；只监听回环请使用下方「仅本机调试」");
      }
      const next = await startMobileGateway(bindAddress.trim(), parsedPort);
      setStatus(next);
      toast.success("移动网关已启动");
    });

  const startLoopbackOnly = () =>
    run("start-loopback", async () => {
      const parsedPort = Number.parseInt(port, 10) || 8787;
      const next = await startMobileGateway(null, parsedPort);
      setStatus(next);
      setBindAddress("");
      toast.info("已只在回环地址上启动，手机无法连接");
    });

  const stop = () =>
    run("stop", async () => {
      const next = await stopMobileGateway();
      setStatus(next);
      setPairing(null);
      toast.info("移动网关已停止");
    });

  const beginPairing = () =>
    run("pairing", async () => {
      const next = await createMobilePairing();
      setPairing(next);
      toast.info("配对二维码已生成，10 分钟内有效");
    });

  const approve = (pendingId: string) =>
    run(`approve-${pendingId}`, async () => {
      const device = await approveMobilePairing(pendingId);
      toast.success(`已允许「${device.name}」接入`);
      await refresh();
    });

  const deny = (pendingId: string) =>
    run(`deny-${pendingId}`, async () => {
      await denyMobilePairing(pendingId);
      await refresh();
    });

  const revoke = (device: MobileDeviceView) =>
    run(`revoke-${device.id}`, async () => {
      await revokeMobileDevice(device.id);
      toast.success(`已撤销「${device.name}」`);
      await refresh();
    });

  const toggleCapability = (capability: MobileCapability) =>
    run(`capability-${capability}`, async () => {
      const next = granted.has(capability)
        ? status?.capabilities.filter((item) => item !== capability) ?? []
        : [...(status?.capabilities ?? []), capability];
      const updated = await setMobileCapabilities(next);
      setStatus(updated);
      toast.info(
        next.includes(capability)
          ? `已向手机开放「${CAPABILITY_LABELS[capability]}」`
          : `已关闭「${CAPABILITY_LABELS[capability]}」`,
      );
    });

  const copyUri = async () => {
    if (!pairing?.uri) return;
    try {
      await navigator.clipboard.writeText(pairing.uri);
      toast.success("配对链接已复制");
    } catch {
      toast.error("复制失败，请手动选择文本");
    }
  };

  return (
    <div className="mobile-settings">
      <header className="mobile-settings__header">
        <div>
          <h2>
            <Smartphone size={16} aria-hidden /> 移动设备
          </h2>
          <p>
            手机只承担控制面：查看会话、发送消息、停止 Turn、处理审批和回答问题。
            文件浏览、终端、插件与供应商配置继续留在桌面端。
          </p>
        </div>
        <button type="button" className="mobile-settings__ghost" onClick={() => void refresh()}>
          <RefreshCw size={14} aria-hidden /> 刷新
        </button>
      </header>

      {error ? (
        <div className="mobile-settings__error" role="alert">
          <AlertTriangle size={15} aria-hidden />
          <span>{error}</span>
        </div>
      ) : null}

      <section className="mobile-settings__card">
        <div className="mobile-settings__card-head">
          <h3>
            {status?.running ? (
              <CheckCircle2 size={15} className="is-ok" aria-hidden />
            ) : (
              <XCircle size={15} className="is-off" aria-hidden />
            )}
            局域网访问
          </h3>
          <span className={status?.running ? "mobile-settings__pill is-on" : "mobile-settings__pill"}>
            {status?.running
              ? `${status.scheme}://${status.host}:${status.port}`
              : "未启动"}
          </span>
        </div>

        <div className="mobile-settings__grid">
          <label className="mobile-settings__field">
            <span>监听地址</span>
            <input
              type="text"
              list="mobile-bind-addresses"
              placeholder="留空仅本机回环"
              autoComplete="off"
              spellCheck={false}
              value={bindAddress}
              onChange={(event) => setBindAddress(event.target.value)}
              disabled={status?.running}
            />
            {/* 用 input + datalist 而不是 select：自动探测只能给出默认出口网卡的地址，
                隧道地址（如 WireGuard 的 10.8.0.2）不会出现在候选里，必须允许手填。 */}
            <datalist id="mobile-bind-addresses">
              {lanAddresses.map((address) => (
                <option key={address} value={address} />
              ))}
            </datalist>
          </label>
          <label className="mobile-settings__field">
            <span>端口</span>
            <input
              type="number"
              min={1}
              max={65535}
              value={port}
              onChange={(event) => setPort(event.target.value)}
              disabled={status?.running}
            />
          </label>
        </div>

        {!status?.running ? (
          <p className="mobile-settings__hint">
            可填写上面自动探测到的地址，也可直接输入隧道地址（例如 WireGuard 的 10.8.0.2）。只接受回环或私网地址。
          </p>
        ) : null}

        <div className="mobile-settings__actions">
          {status?.running ? (
            <button
              type="button"
              className="mobile-settings__danger"
              onClick={() => void stop()}
              disabled={busy !== null}
            >
              {busy === "stop" ? <LoaderCircle size={14} className="is-spin" /> : <Square size={14} />}
              停止
            </button>
          ) : (
            <>
              <button
                type="button"
                className="mobile-settings__primary"
                onClick={() => void start()}
                disabled={busy !== null || !bindAddress.trim()}
              >
                {busy === "start" ? (
                  <LoaderCircle size={14} className="is-spin" />
                ) : (
                  <Play size={14} />
                )}
                开启局域网访问
              </button>
              <button
                type="button"
                className="mobile-settings__ghost"
                onClick={() => void startLoopbackOnly()}
                disabled={busy !== null}
              >
                <Wifi size={14} aria-hidden /> 仅本机调试
              </button>
            </>
          )}
        </div>

        {!lanAddresses.length && !status?.running ? (
          <p className="mobile-settings__hint">
            没有自动探测到本机私网地址。请确认电脑已连接局域网，或稍后重试。
          </p>
        ) : null}

        {status?.running && status.scheme === "https" ? (
          <div className="mobile-settings__fingerprint">
            <div className="mobile-settings__fingerprint-head">
              <ShieldCheck size={14} aria-hidden />
              <span>本机证书指纹（手机首次连接必须核对）</span>
            </div>
            <code>{status.fingerprint ?? "—"}</code>
          </div>
        ) : null}

        {status?.running && status.scheme === "http" ? (
          <p className="mobile-settings__hint is-warning">
            当前只监听回环地址，未启用 TLS。这个模式仅用于本机调试，手机无法接入。
          </p>
        ) : null}
      </section>

      <section className="mobile-settings__card">
        <div className="mobile-settings__card-head">
          <h3>
            <QrCode size={15} aria-hidden /> 配对
          </h3>
          <button
            type="button"
            className="mobile-settings__primary"
            onClick={() => void beginPairing()}
            disabled={busy !== null || !status?.running}
          >
            {busy === "pairing" ? <LoaderCircle size={14} className="is-spin" /> : <Link2 size={14} />}
            {pairing ? "重新生成" : "生成配对二维码"}
          </button>
        </div>

        {!status?.running ? (
          <p className="mobile-settings__hint">先启动局域网访问，才能生成配对二维码。</p>
        ) : null}

        {pairing ? (
          <div className="mobile-settings__pairing">
            <div className="mobile-settings__qr">
              {qrDataUrl ? (
                <img src={qrDataUrl} alt="配对二维码" width={200} height={200} />
              ) : (
                <div className="mobile-settings__qr-placeholder">二维码生成中…</div>
              )}
            </div>
            <div className="mobile-settings__pairing-body">
              <div className="mobile-settings__code">
                <span>人工校验码</span>
                <strong>{pairing.code}</strong>
              </div>
              <ol className="mobile-settings__steps">
                <li>手机连接同一个局域网，扫描左侧二维码。</li>
                <li>浏览器提示证书不受信任时，先核对上面的证书指纹再继续。</li>
                <li>在手机上输入人工校验码并提交。</li>
                <li>回到这里确认设备名称，确认后才发放设备凭据。</li>
              </ol>
              <div className="mobile-settings__uri">
                <code>{pairing.uri}</code>
                <button type="button" className="mobile-settings__ghost" onClick={() => void copyUri()}>
                  <Copy size={13} aria-hidden /> 复制
                </button>
              </div>
              <p className="mobile-settings__hint">
                有效至 {formatTime(pairing.expiresAtMs)}。挑战是一次性的：用掉或过期后必须重新生成。
              </p>
            </div>
          </div>
        ) : null}

        {pending.length ? (
          <div className="mobile-settings__pending">
            <h4>等待确认的设备</h4>
            {pending.map((request) => (
              <div key={request.id} className="mobile-settings__pending-row">
                <div>
                  <strong>{request.deviceName}</strong>
                  <span className="mobile-settings__muted">
                    {request.platform ?? "未知平台"} · {formatTime(request.createdAtMs)}
                  </span>
                </div>
                <div className="mobile-settings__row-actions">
                  <button
                    type="button"
                    className="mobile-settings__primary"
                    onClick={() => void approve(request.id)}
                    disabled={busy !== null}
                  >
                    <CheckCircle2 size={13} aria-hidden /> 允许
                  </button>
                  <button
                    type="button"
                    className="mobile-settings__ghost"
                    onClick={() => void deny(request.id)}
                    disabled={busy !== null}
                  >
                    <XCircle size={13} aria-hidden /> 拒绝
                  </button>
                </div>
              </div>
            ))}
          </div>
        ) : null}
      </section>

      <section className="mobile-settings__card">
        <div className="mobile-settings__card-head">
          <h3>
            <Lock size={15} aria-hidden /> 已授权设备
          </h3>
          <span className="mobile-settings__pill">{devices.filter((d) => !d.revoked).length} 台在用</span>
        </div>
        {devices.length ? (
          <div className="mobile-settings__devices">
            {devices.map((device) => (
              <div key={device.id} className="mobile-settings__device">
                <div>
                  <strong>{device.name}</strong>
                  <span className="mobile-settings__muted">
                    {device.platform ?? "未知平台"} · 最后在线 {formatTime(device.lastSeenAtMs)}
                  </span>
                </div>
                <div className="mobile-settings__row-actions">
                  <span className={device.revoked ? "mobile-settings__pill" : "mobile-settings__pill is-on"}>
                    {deviceStatusLabel(device)}
                  </span>
                  {device.revoked ? null : (
                    <button
                      type="button"
                      className="mobile-settings__danger"
                      onClick={() => void revoke(device)}
                      disabled={busy !== null}
                    >
                      <Trash2 size={13} aria-hidden /> 撤销
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <p className="mobile-settings__hint">还没有配对过任何设备。</p>
        )}
      </section>

      <section className="mobile-settings__card">
        <div className="mobile-settings__card-head">
          <h3>
            <ShieldCheck size={15} aria-hidden /> 手机端可用能力
          </h3>
        </div>
        <ul className="mobile-settings__capabilities">
          {(Object.keys(CAPABILITY_LABELS) as MobileCapability[]).map((capability) => {
            const implemented = IMPLEMENTED_CAPABILITIES.includes(capability);
            const enabled = granted.has(capability);
            const label = CAPABILITY_LABELS[capability];
            if (!implemented) {
              return (
                <li key={capability} className="is-locked">
                  <Lock size={14} aria-hidden />
                  <span>{label}</span>
                  <em>本期未实现</em>
                </li>
              );
            }
            return (
              <li key={capability} className={enabled ? "is-on" : ""}>
                <button
                  type="button"
                  className="mobile-settings__capability-toggle"
                  onClick={() => void toggleCapability(capability)}
                  disabled={busy !== null}
                  aria-pressed={enabled}
                >
                  {busy === `capability-${capability}` ? (
                    <LoaderCircle size={14} className="is-spin" aria-hidden />
                  ) : enabled ? (
                    <CheckCircle2 size={14} aria-hidden />
                  ) : (
                    <XCircle size={14} aria-hidden />
                  )}
                  <span>{label}</span>
                  <em>{enabled ? "已开放" : "已关闭"}</em>
                </button>
              </li>
            );
          })}
        </ul>
        <p className="mobile-settings__hint">
          关闭「查看会话与发送消息」会让手机彻底无法使用。供应商配置、插件管理、MCP 配置、密钥管理和系统级设置不向手机开放。
        </p>
      </section>
    </div>
  );
}
