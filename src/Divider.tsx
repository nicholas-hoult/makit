import { useEffect, useRef } from "react";
import { terminalManager } from "./TerminalManager";

type DividerProps = {
  direction: "horizontal" | "vertical";
  onResize: (delta: number) => void;
};

export function Divider({ direction, onResize }: DividerProps) {
  const divRef = useRef<HTMLDivElement>(null);
  const isVertical = direction === "vertical";

  useEffect(() => {
    const div = divRef.current;
    if (!div) return;

    let dragging = false;
    let startPos = 0;

    const onPointerDown = (e: PointerEvent) => {
      e.preventDefault();
      div.setPointerCapture(e.pointerId);
      dragging = true;
      startPos = isVertical ? e.clientX : e.clientY;
      div.classList.add("is-dragging");
      document.body.classList.add("pane-resizing");
    };

    const onPointerMove = (e: PointerEvent) => {
      if (!dragging) return;
      const currentPos = isVertical ? e.clientX : e.clientY;
      const delta = currentPos - startPos;
      onResize(delta);
      startPos = currentPos;
    };

    const onPointerUp = (e: PointerEvent) => {
      if (!dragging) return;
      dragging = false;
      if (div.hasPointerCapture(e.pointerId)) {
        div.releasePointerCapture(e.pointerId);
      }
      div.classList.remove("is-dragging");
      document.body.classList.remove("pane-resizing");
      terminalManager.flushResize(); // 松手：延后的列数重排立即做掉（#203）
    };

    const onPointerCancel = () => {
      dragging = false;
      div.classList.remove("is-dragging");
      document.body.classList.remove("pane-resizing");
      terminalManager.flushResize(); // 松手：延后的列数重排立即做掉（#203）
    };

    div.addEventListener("pointerdown", onPointerDown);
    div.addEventListener("pointermove", onPointerMove);
    div.addEventListener("pointerup", onPointerUp);
    div.addEventListener("pointercancel", onPointerCancel);

    return () => {
      div.removeEventListener("pointerdown", onPointerDown);
      div.removeEventListener("pointermove", onPointerMove);
      div.removeEventListener("pointerup", onPointerUp);
      div.removeEventListener("pointercancel", onPointerCancel);
    };
  }, [isVertical, onResize]);

  return (
    <div
      ref={divRef}
      className={`pane-divider ${isVertical ? "is-vertical" : "is-horizontal"}`}
      style={{
        flex: "none",
        cursor: isVertical ? "col-resize" : "row-resize",
        width: isVertical ? "10px" : "auto",
        height: isVertical ? "auto" : "10px",
        position: "relative",
      }}
    />
  );
}
