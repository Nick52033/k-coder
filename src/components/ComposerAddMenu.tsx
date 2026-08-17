import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  Boxes,
  Menu,
  Network,
  Puzzle,
  Workflow,
} from "lucide-react";

type AddSettingsSection = "plugins" | "mcp" | "miniapps" | "workflows";

interface ComposerAddMenuProps {
  onOpenSettings: (section: AddSettingsSection) => void;
}

export function ComposerAddMenu({
  onOpenSettings,
}: ComposerAddMenuProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [menuPosition, setMenuPosition] = useState({ left: 12, bottom: 48, width: 260 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  function updateMenuPosition() {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const viewportPadding = 12;
    const width = Math.min(260, Math.max(220, window.innerWidth - viewportPadding * 2));
    setMenuPosition({
      left: Math.max(
        viewportPadding,
        Math.min(rect.left, window.innerWidth - width - viewportPadding),
      ),
      bottom: Math.max(viewportPadding, window.innerHeight - rect.top + 7),
      width,
    });
  }

  useEffect(() => {
    if (!isOpen) return;

    function closeAndRestoreFocus() {
      setIsOpen(false);
      triggerRef.current?.focus();
    }

    function handlePointerDown(event: MouseEvent) {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !menuRef.current?.contains(target)) {
        setIsOpen(false);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeAndRestoreFocus();
    }

    updateMenuPosition();
    requestAnimationFrame(() => {
      menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    });
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    window.addEventListener("resize", updateMenuPosition);
    window.addEventListener("scroll", updateMenuPosition, true);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("resize", updateMenuPosition);
      window.removeEventListener("scroll", updateMenuPosition, true);
    };
  }, [isOpen]);

  function runAction(action: () => void) {
    setIsOpen(false);
    action();
  }

  function handleMenuKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [],
    );
    if (!items.length) return;
    event.preventDefault();
    const activeIndex = items.findIndex((item) => item === document.activeElement);
    if (event.key === "Home") items[0].focus();
    else if (event.key === "End") items[items.length - 1].focus();
    else {
      const direction = event.key === "ArrowDown" ? 1 : -1;
      const nextIndex = (Math.max(0, activeIndex) + direction + items.length) % items.length;
      items[nextIndex].focus();
    }
  }

  return (
    <div className="composer-add">
      <button
        ref={triggerRef}
        className="composer-add-trigger"
        type="button"
        aria-label="更多操作"
        aria-expanded={isOpen}
        aria-haspopup="menu"
        aria-controls={isOpen ? menuId : undefined}
        title="更多操作"
        onClick={() => {
          if (!isOpen) updateMenuPosition();
          setIsOpen((open) => !open);
        }}
      >
        <Menu size={17} aria-hidden="true" />
      </button>

      {isOpen && createPortal(
        <div
          ref={menuRef}
          id={menuId}
          className="composer-add-menu"
          role="menu"
          aria-label="添加内容"
          style={menuPosition}
          onKeyDown={handleMenuKeyDown}
        >
          <AddMenuItem icon={<Puzzle size={17} />} label="添加插件" onClick={() => runAction(() => onOpenSettings("plugins"))} />
          <AddMenuItem icon={<Network size={17} />} label="添加 MCP" onClick={() => runAction(() => onOpenSettings("mcp"))} />
          <AddMenuItem icon={<Boxes size={17} />} label="添加小程序" onClick={() => runAction(() => onOpenSettings("miniapps"))} />
          <AddMenuItem icon={<Workflow size={17} />} label="添加 Workflow" onClick={() => runAction(() => onOpenSettings("workflows"))} />
        </div>,
        document.body,
      )}
    </div>
  );
}

function AddMenuItem({
  icon,
  label,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      className="composer-add-menu-item"
      type="button"
      role="menuitem"
      onClick={onClick}
    >
      <span aria-hidden="true">{icon}</span>
      <strong>{label}</strong>
    </button>
  );
}
