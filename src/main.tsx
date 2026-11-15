import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauriRuntime, triggerFullscreenCapture } from "./lib/tauri-client";
import { useGalleryStore } from "./stores/useGalleryStore";
import { GalleryApp } from "./windows/gallery/GalleryApp";
import { OverlayApp } from "./windows/overlay/OverlayApp";
import "./styles/globals.css";

const RootRouter: React.FC = () => {
  const [windowLabel, setWindowLabel] = useState<"main" | "overlay">(() => {
    if (isTauriRuntime()) {
      try {
        const lbl = getCurrentWindow().label;
        if (lbl === "overlay") return "overlay";
      } catch {
        // Default to main
      }
    }
    const params = new URLSearchParams(window.location.search);
    return params.get("window") === "overlay" ? "overlay" : "main";
  });

  const { fetchCaptures } = useGalleryStore();

  // Support Alt+S, Alt+Shift+S, and Alt+G in Browser Dev Preview mode too
  useEffect(() => {
    if (isTauriRuntime()) return;

    const handleGlobalKey = async (e: KeyboardEvent) => {
      if (e.altKey && e.shiftKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        await triggerFullscreenCapture();
        await fetchCaptures();
      } else if (e.altKey && !e.shiftKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        setWindowLabel("overlay");
      } else if (e.altKey && e.key.toLowerCase() === "g") {
        e.preventDefault();
        setWindowLabel("main");
        await fetchCaptures();
      }
    };

    window.addEventListener("keydown", handleGlobalKey);
    return () => window.removeEventListener("keydown", handleGlobalKey);
  }, [fetchCaptures]);

  if (windowLabel === "overlay") {
    return (
      <OverlayApp
        onCloseInBrowserPreview={() => {
          setWindowLabel("main");
          fetchCaptures();
        }}
      />
    );
  }

  return (
    <GalleryApp
      onOpenOverlayPreview={() => {
        setWindowLabel("overlay");
      }}
    />
  );
};

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <RootRouter />
  </React.StrictMode>
);
