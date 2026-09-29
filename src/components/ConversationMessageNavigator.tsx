import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { Bookmark } from "lucide-react";
import type { ConversationMessage } from "../types/runtime";
import "./ConversationMessageNavigator.css";

interface ConversationMessageNavigatorProps {
  threadId: string | null;
  messages: ConversationMessage[];
  scrollerRef: RefObject<HTMLDivElement | null>;
  stageRef: RefObject<HTMLDivElement | null>;
}

interface MessageAnchor {
  id: string;
  title: string;
}

interface PositionedAnchor extends MessageAnchor {
  top: number;
  contentTop: number;
}

const BOOKMARKS_KEY_PREFIX = "kcoder_message_bookmarks_v1:";

function readBookmarks(threadId: string | null): Set<string> {
  if (!threadId) return new Set();
  try {
    const value: unknown = JSON.parse(localStorage.getItem(`${BOOKMARKS_KEY_PREFIX}${threadId}`) ?? "[]");
    return Array.isArray(value) ? new Set(value.filter((item): item is string => typeof item === "string")) : new Set();
  } catch {
    return new Set();
  }
}

function compactText(value: string, limit: number): string {
  const compact = value.replace(/\s+/g, " ").trim();
  return compact.length > limit ? `${compact.slice(0, limit - 1).trimEnd()}…` : compact;
}

function createMessageAnchors(messages: ConversationMessage[]): MessageAnchor[] {
  return messages.flatMap((message) => {
    if (message.role !== "user") return [];
    const prompt = message.text.trim()
      || (message.attachments?.length ? `发送了 ${message.attachments.length} 个附件` : "发送了一条消息");
    return [{
      id: message.id,
      title: compactText(prompt, 120),
    }];
  });
}

export function ConversationMessageNavigator({
  threadId,
  messages,
  scrollerRef,
  stageRef,
}: ConversationMessageNavigatorProps) {
  const anchors = useMemo(() => createMessageAnchors(messages), [messages]);
  const anchorSignature = anchors.map((anchor) => anchor.id).join("\u0000");
  const anchorsRef = useRef(anchors);
  anchorsRef.current = anchors;
  const positionedAnchorsRef = useRef<PositionedAnchor[]>([]);
  const [positionedAnchors, setPositionedAnchors] = useState<PositionedAnchor[]>([]);
  const [activeAnchorId, setActiveAnchorId] = useState<string | null>(null);
  const [hoveredAnchorId, setHoveredAnchorId] = useState<string | null>(null);
  const [focusedAnchorId, setFocusedAnchorId] = useState<string | null>(null);
  const [bookmarkedIds, setBookmarkedIds] = useState<Set<string>>(() => readBookmarks(threadId));

  useEffect(() => {
    setBookmarkedIds(readBookmarks(threadId));
    setActiveAnchorId(null);
    setHoveredAnchorId(null);
    setFocusedAnchorId(null);
  }, [threadId]);

  useEffect(() => {
    setPositionedAnchors((previous) => {
      if (!previous.length) return previous;
      const latestById = new Map(anchorsRef.current.map((anchor) => [anchor.id, anchor]));
      const next = previous.map((positioned) => ({
        ...positioned,
        ...(latestById.get(positioned.id) ?? {}),
      }));
      positionedAnchorsRef.current = next;
      return next;
    });
  }, [anchors]);

  useEffect(() => {
    const scroller = scrollerRef.current;
    const stage = stageRef.current;
    if (!scroller || !stage || anchors.length < 2) {
      setPositionedAnchors([]);
      return;
    }

    let frame = 0;
    const findMessage = (id: string) => Array.from(
      scroller.querySelectorAll<HTMLElement>("[data-message-id]"),
    ).find((element) => element.dataset.messageId === id);

    const measure = () => {
      frame = 0;
      const stageHeight = stage.clientHeight;
      const totalHeight = Math.max(scroller.scrollHeight, 1);
      const scrollBounds = scroller.getBoundingClientRect();
      if (!stageHeight) return;

      const measured = anchorsRef.current.flatMap((anchor) => {
        const element = findMessage(anchor.id);
        if (!element) return [];
        const bounds = element.getBoundingClientRect();
        const contentTop = scroller.scrollTop + bounds.top - scrollBounds.top + bounds.height / 2;
        const top = Math.min(stageHeight - 8, Math.max(8, (contentTop / totalHeight) * stageHeight));
        return [{ ...anchor, contentTop, top }];
      });
      const firstTop = measured[0]?.top ?? 0;
      const lastTop = measured[measured.length - 1]?.top ?? firstTop;
      const rawSpread = lastTop - firstTop;
      const maxClusterSpread = Math.min(stageHeight * 0.36, Math.max(56, (measured.length - 1) * 8));
      const compression = rawSpread > maxClusterSpread ? maxClusterSpread / rawSpread : 1;
      const center = (firstTop + lastTop) / 2;
      const next = measured.map((anchor) => ({
        ...anchor,
        top: center + (anchor.top - center) * compression,
      }));
      setPositionedAnchors((previous) => {
        if (previous.length === next.length && previous.every((item, index) =>
          item.id === next[index]?.id
          && Math.abs(item.top - (next[index]?.top ?? 0)) < 0.5
          && item.title === next[index]?.title
        )) {
          positionedAnchorsRef.current = previous;
          return previous;
        }
        positionedAnchorsRef.current = next;
        return next;
      });
    };

    const scheduleMeasure = () => {
      if (!frame) frame = window.requestAnimationFrame(measure);
    };
    const updateActiveAnchor = () => {
      const center = scroller.scrollTop + scroller.clientHeight / 2;
      const currentAnchors = positionedAnchorsRef.current;
      const previousAnchors = currentAnchors.filter((anchor) => anchor.contentTop <= center);
      const current = previousAnchors[previousAnchors.length - 1] ?? currentAnchors[0];
      setActiveAnchorId(current?.id ?? null);
    };

    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(scheduleMeasure);
    resizeObserver?.observe(stage);
    resizeObserver?.observe(scroller);
    const messageList = scroller.querySelector<HTMLElement>(".message-list");
    if (messageList) resizeObserver?.observe(messageList);
    for (const anchor of anchors) {
      const element = findMessage(anchor.id);
      if (element) resizeObserver?.observe(element);
    }
    scroller.addEventListener("scroll", updateActiveAnchor, { passive: true });
    window.addEventListener("resize", scheduleMeasure);
    scheduleMeasure();

    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      resizeObserver?.disconnect();
      scroller.removeEventListener("scroll", updateActiveAnchor);
      window.removeEventListener("resize", scheduleMeasure);
    };
    // Positioning depends on stable message ids; ResizeObserver handles streaming and layout changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId, anchorSignature, anchors.length, scrollerRef, stageRef]);

  useEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller || positionedAnchors.length < 2) return;
    const center = scroller.scrollTop + scroller.clientHeight / 2;
    const previousAnchors = positionedAnchors.filter((anchor) => anchor.contentTop <= center);
    const current = previousAnchors[previousAnchors.length - 1] ?? positionedAnchors[0];
    setActiveAnchorId(current?.id ?? null);
  }, [positionedAnchors, scrollerRef]);

  if (!threadId || anchors.length < 2 || positionedAnchors.length < 2) return null;

  const stageHeight = stageRef.current?.clientHeight ?? 0;
  const firstMarkerTop = Math.min(...positionedAnchors.map((anchor) => anchor.top));
  const lastMarkerTop = Math.max(...positionedAnchors.map((anchor) => anchor.top));
  const railTop = Math.max(8, firstMarkerTop - 6);
  const railBottom = Math.min(stageHeight - 8, lastMarkerTop + 6);

  const jumpToMessage = (id: string) => {
    const scroller = scrollerRef.current;
    const element = scroller && Array.from(
      scroller.querySelectorAll<HTMLElement>("[data-message-id]"),
    ).find((candidate) => candidate.dataset.messageId === id);
    element?.scrollIntoView({
      behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth",
      block: "center",
    });
  };

  const toggleBookmark = (id: string) => {
    setBookmarkedIds((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      try {
        localStorage.setItem(`${BOOKMARKS_KEY_PREFIX}${threadId}`, JSON.stringify([...next]));
      } catch {
        // Keep the current session usable when local storage is unavailable.
      }
      return next;
    });
  };

  return (
    <nav className="conversation-message-navigator" aria-label="用户消息导航">
      <div
        className="conversation-message-navigator__rail"
        style={{ top: `${railTop}px`, height: `${Math.max(12, railBottom - railTop)}px` }}
        aria-hidden="true"
      />
      {positionedAnchors.map((anchor) => {
        const isBookmarked = bookmarkedIds.has(anchor.id);
        const isActive = activeAnchorId === anchor.id;
        const isPreviewVisible = hoveredAnchorId
          ? hoveredAnchorId === anchor.id
          : focusedAnchorId === anchor.id;
        const previewHeight = 60;
        const previewTop = Math.min(
          Math.max(8, anchor.top - previewHeight / 2),
          Math.max(8, stageHeight - previewHeight - 8),
        ) - anchor.top;
        return (
          <div
            className="conversation-message-anchor"
            key={anchor.id}
            style={{ top: `${anchor.top}px` }}
            onPointerEnter={() => setHoveredAnchorId(anchor.id)}
            onPointerLeave={() => setHoveredAnchorId((current) => current === anchor.id ? null : current)}
            onFocus={() => setFocusedAnchorId(anchor.id)}
            onBlur={(event) => {
              if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
                setFocusedAnchorId((current) => current === anchor.id ? null : current);
              }
            }}
          >
            <button
              type="button"
              className={`conversation-message-anchor__marker${isActive ? " is-active" : ""}${isBookmarked ? " is-bookmarked" : ""}`}
              aria-label={`跳转到用户消息：${anchor.title}`}
              title={anchor.title}
              onClick={() => jumpToMessage(anchor.id)}
            />
            {isPreviewVisible ? (
              <div
                className="conversation-message-anchor__preview"
                style={{ top: `${previewTop}px` }}
                aria-label="用户提问"
              >
                <div className="conversation-message-anchor__heading">
                  <strong>{anchor.title}</strong>
                  <button
                    type="button"
                    className={`conversation-message-anchor__bookmark${isBookmarked ? " is-saved" : ""}`}
                    aria-label={isBookmarked ? "取消收藏这条消息" : "收藏这条消息"}
                    title={isBookmarked ? "取消收藏" : "收藏消息"}
                    aria-pressed={isBookmarked}
                    onClick={() => toggleBookmark(anchor.id)}
                  >
                    <Bookmark size={15} aria-hidden="true" />
                  </button>
                </div>
              </div>
            ) : null}
          </div>
        );
      })}
    </nav>
  );
}
