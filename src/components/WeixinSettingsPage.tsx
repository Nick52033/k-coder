import { useEffect, useState } from "react";
import QRCode from "qrcode";
import { CheckCircle2, LoaderCircle, MessageCircle, Play, Square } from "lucide-react";
import { listThreads, startWeixin, stopWeixin, weixinStatus } from "../api/runtime";
import type { ThreadSummary, WeixinStatus } from "../types/runtime";
import { useToast } from "./Toast";

export function WeixinSettingsPage({ activeThreadId }: { activeThreadId: string | null }) {
  const toast = useToast(); const [status,setStatus]=useState<WeixinStatus|null>(null); const [threads,setThreads]=useState<ThreadSummary[]>([]); const [threadId,setThreadId]=useState(activeThreadId ?? ""); const [remember,setRemember]=useState(false); const [autoConnect,setAutoConnect]=useState(false); const [qr,setQr]=useState(""); const [busy,setBusy]=useState(false);
  const refresh=async()=>{ try { const [s,t]=await Promise.all([weixinStatus(),listThreads()]); setStatus(s); setThreads(t.filter(x=>!x.archived)); setThreadId(v=>v||s.threadId||activeThreadId||""); if(s.qrImageContent) setQr(await QRCode.toDataURL(s.qrImageContent,{width:240})); else setQr(""); } catch(e){ toast.error(String(e)); } };
  useEffect(()=>{void refresh(); const id=window.setInterval(()=>void refresh(),1500); return()=>window.clearInterval(id)},[]);
  const start=async()=>{ if(!threadId) return toast.error("请选择桌面会话"); setBusy(true); try { setStatus(await startWeixin(threadId,remember,autoConnect)); toast.success("微信扫码连接已启动"); } catch(e){toast.error(String(e))} finally{setBusy(false)} };
  const stop=async()=>{setBusy(true);try{setStatus(await stopWeixin());toast.info("微信接入已停止")}catch(e){toast.error(String(e))}finally{setBusy(false)}};
  return <div className="mobile-settings"><header className="mobile-settings__header"><div><h2><MessageCircle size={16}/> 微信接入</h2><p>扫码后，微信文本会进入选定的桌面会话，并沿用该会话的模型、工作区和权限。</p></div></header>
    <section className="mobile-settings__card"><label className="mobile-settings__field"><span>绑定桌面会话</span><select value={threadId} onChange={e=>setThreadId(e.target.value)} disabled={status?.running}>{threads.map(t=><option key={t.id} value={t.id}>{t.title||t.id}</option>)}</select></label>
      <label className="mobile-settings__check"><input type="checkbox" checked={remember} onChange={e=>setRemember(e.target.checked)} disabled={status?.running}/> 安全记住登录凭据</label>
      <label className="mobile-settings__check"><input type="checkbox" checked={autoConnect} onChange={e=>setAutoConnect(e.target.checked)} disabled={status?.running}/> 应用启动时自动连接</label>
      <p className="mobile-settings__hint">支持：发送文本、<code>/stop 编号</code> 停止当前任务。审批和问题回答会显示编号，过期或跨会话指令会被拒绝。</p>
      <div className="mobile-settings__actions">{status?.running?<button className="mobile-settings__danger" onClick={()=>void stop()} disabled={busy}><Square size={14}/>停止微信接入</button>:<button className="mobile-settings__primary" onClick={()=>void start()} disabled={busy||!threadId}>{busy?<LoaderCircle className="is-spin" size={14}/>:<Play size={14}/>}扫码连接</button>}</div>
      {status?.phase?<p className="mobile-settings__hint" role="status">{status.phase}</p>:null}{status?.error?<p className="mobile-settings__error">{status.error}</p>:null}{qr?<img className="mobile-settings__qr" src={qr} alt="微信扫码二维码"/>:null}{status?.owner?<p className="mobile-settings__hint"><CheckCircle2 size={14}/> 已连接微信账号 {status.owner}</p>:null}
    </section></div>;
}
