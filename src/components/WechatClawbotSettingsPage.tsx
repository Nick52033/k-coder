import { useCallback, useEffect, useRef, useState } from "react";
import QRCode from "qrcode";
import { Check, LoaderCircle, QrCode, RefreshCw, ShieldAlert, Trash2, Unplug } from "lucide-react";
import {
  approveWechatClawbotSender,
  disconnectWechatClawbot,
  errorMessage,
  revokeWechatClawbotSender,
  startWechatClawbotLogin,
  wechatClawbotLoginStatus,
  wechatClawbotStatus,
} from "../api/runtime";
import type { WechatClawbotStatus } from "../types/runtime";
import { useToast } from "./Toast";

export function WechatClawbotSettingsPage() {
  const toast = useToast();
  const [status, setStatus] = useState<WechatClawbotStatus | null>(null);
  const [qr, setQr] = useState("");
  const [qrImage, setQrImage] = useState("");
  const [phase, setPhase] = useState("");
  const [message, setMessage] = useState("");
  const [verifyCode, setVerifyCode] = useState("");
  const [busy, setBusy] = useState(false);
  const loginIdRef = useRef<string | null>(null);

  const refresh = useCallback(async () => {
    try { setStatus(await wechatClawbotStatus()); } catch (error) { setMessage(errorMessage(error)); }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  useEffect(() => {
    if (!qr) { setQrImage(""); return; }
    let stale = false;
    QRCode.toDataURL(qr, { width: 240, margin: 1 }).then((image) => { if (!stale) setQrImage(image); });
    return () => { stale = true; };
  }, [qr]);

  useEffect(() => {
    if (!loginIdRef.current || phase === "connected" || phase === "error" || phase === "expired") return;
    const loginId = loginIdRef.current;
    const timer = window.setInterval(() => {
      void wechatClawbotLoginStatus(loginId).then((next) => {
        setPhase(next.phase);
        if (next.message) setMessage(next.message);
        if (next.phase === "connected") {
          setQr("");
          loginIdRef.current = null;
          void refresh();
          toast.success("微信 ClawBot 已连接");
        }
      }).catch((error) => { setMessage(errorMessage(error)); });
    }, 1000);
    return () => window.clearInterval(timer);
  }, [phase, refresh, toast]);

  async function beginLogin() {
    setBusy(true); setMessage(""); setVerifyCode("");
    try {
      const login = await startWechatClawbotLogin();
      loginIdRef.current = login.loginId;
      setQr(login.qrContent);
      setPhase("waiting");
    } catch (error) { setMessage(errorMessage(error)); } finally { setBusy(false); }
  }

  async function approve(senderKey: string) {
    setBusy(true);
    try { await approveWechatClawbotSender(senderKey); await refresh(); toast.success("已批准该微信发送方"); }
    catch (error) { setMessage(errorMessage(error)); }
    finally { setBusy(false); }
  }

  async function revoke(senderKey: string) {
    setBusy(true);
    try { await revokeWechatClawbotSender(senderKey); await refresh(); toast.info("已撤销该发送方"); }
    catch (error) { setMessage(errorMessage(error)); }
    finally { setBusy(false); }
  }

  async function disconnect() {
    setBusy(true);
    try { setStatus(await disconnectWechatClawbot()); setQr(""); setPhase(""); toast.info("微信 ClawBot 已断开"); }
    catch (error) { setMessage(errorMessage(error)); }
    finally { setBusy(false); }
  }

  return (
    <section className="settings-page" aria-labelledby="wechat-clawbot-title">
      <div className="settings-page-header">
        <div><p className="settings-eyebrow">实验接入</p><h3 id="wechat-clawbot-title">微信 ClawBot</h3></div>
        <span className={`status-badge ${status?.connected ? "status-badge--success" : ""}`}>{status?.connected ? (status.polling ? "已连接" : "已暂停") : "未连接"}</span>
      </div>
      <p className="settings-page-description">通过本机直接连接 iLink Bot。扫码后，收到的消息需要先在这里批准发送方，再进入当前项目下专用的 k-Coder 会话。</p>
      <div className="settings-warning-banner" role="note">
        <ShieldAlert size={16} />
        <span>腾讯公开的独立桌面客户端许可、App ID 和发布要求尚未得到明确说明。此接入使用公开 iLink 客户端行为，服务端兼容性可能变化。首版只处理文字私聊；群聊和媒体消息会被忽略。</span>
      </div>
      {message && <div className="settings-inline-error" role="alert">{message}</div>}

      {!status?.connected ? (
        <div className="wechat-clawbot-login">
          <button className="primary-button" type="button" disabled={busy || Boolean(qr)} onClick={() => void beginLogin()}>
            {busy ? <LoaderCircle size={15} className="spin" /> : qr ? <RefreshCw size={15} /> : <QrCode size={15} />}
            {qr ? "等待微信扫码…" : "微信扫码连接"}
          </button>
          {qrImage && <figure><img src={qrImage} alt="微信 ClawBot 登录二维码" /><figcaption>{phase === "needs_verification" ? "请输入手机微信上显示的验证码" : phase === "scanned" ? "已扫码，正在确认" : "请用微信扫描二维码并确认"}</figcaption></figure>}
          {phase === "needs_verification" && (
            <form className="wechat-clawbot-verify" onSubmit={(event) => {
              event.preventDefault();
              if (!loginIdRef.current) return;
              setBusy(true);
              void submitVerifyCode(loginIdRef.current, verifyCode).then(() => { setVerifyCode(""); setPhase("verifying"); }).catch((error) => setMessage(errorMessage(error))).finally(() => setBusy(false));
            }}>
              <label>微信验证码<input value={verifyCode} onChange={(event) => setVerifyCode(event.target.value)} maxLength={12} autoComplete="one-time-code" /></label>
              <button className="primary-button" disabled={busy || !verifyCode.trim()}>提交验证码</button>
            </form>
          )}
          {phase === "error" || phase === "expired" ? <button className="secondary-button" type="button" onClick={() => { setQr(""); setPhase(""); loginIdRef.current = null; }}>重新扫码</button> : null}
        </div>
      ) : (
        <div className="wechat-clawbot-connected">
          <p>iLink 长轮询在本机运行。首次私聊发送方会显示为待批准；批准后该发送方才会触发模型调用。</p>
          {status.paused && <p className="settings-warning-text">iLink 暂停了当前账号会话，请断开后重新扫码。</p>}
          <button className="secondary-button danger-button" type="button" disabled={busy} onClick={() => void disconnect()}><Unplug size={15} />断开微信</button>
        </div>
      )}

      {status && (status.pendingSenders.length > 0 || status.approvedSenders.length > 0) && (
        <div className="wechat-clawbot-senders">
          <h4>发送方授权</h4>
          {[...status.pendingSenders, ...status.approvedSenders].map((sender) => (
            <div className="wechat-clawbot-sender" key={sender.senderKey}>
              <div><strong>{sender.approved ? "已批准" : "待批准"}</strong><small>标识 {sender.senderKey}</small>{sender.hasThread && <small>已创建独立会话</small>}</div>
              {sender.approved
                ? <button className="icon-button" type="button" disabled={busy} aria-label="撤销发送方授权" onClick={() => void revoke(sender.senderKey)}><Trash2 size={15} /></button>
                : <button className="primary-button" type="button" disabled={busy} onClick={() => void approve(sender.senderKey)}><Check size={14} />批准</button>}
            </div>
          ))}
        </div>
      )}
      <p className="settings-page-description">机器人消息不会跳过 k-Coder 已有的工具授权和审批；需要你处理审批时，请回到桌面端确认。</p>
    </section>
  );
}

