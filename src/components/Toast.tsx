import { useEffect } from "react";
import { Icon } from "./Icons";

/**
 * 轻提示（画匣 Toast 同款）。
 *
 * 三种语义：info / success / error。错误停得久一点（7 秒）——
 * 报错信息要让人看清，普通提示快速消失就好。
 */

export type ToastKind = "info" | "success" | "error";

export interface ToastData {
  id: number;
  message: string;
  kind: ToastKind;
}

interface Props {
  toast: ToastData;
  onClose: () => void;
}

export function Toast({ toast, onClose }: Props) {
  useEffect(() => {
    const timeout = window.setTimeout(onClose, toast.kind === "error" ? 7000 : 4200);
    return () => window.clearTimeout(timeout);
  }, [toast.id, toast.kind, onClose]);

  return (
    <div className={`toast toast-${toast.kind}`} role="status">
      <Icon name={toast.kind === "error" ? "x" : toast.kind === "success" ? "check" : "info"} size={15} />
      <span className="toast-msg">{toast.message}</span>
      <button className="toast-close" onClick={onClose} title="关掉" aria-label="关掉">
        <Icon name="x" size={12} />
      </button>
    </div>
  );
}
