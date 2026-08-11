import React from 'react';
import { createRoot } from 'react-dom/client';
import { DesktopApp } from './shell';
import '../styles.css';
import './styles.css';

// The window chrome differs per platform and the layout has to match it.
// tauri.conf.json asks for titleBarStyle "Overlay", but that key is
// cfg(target_os = "macos") in tauri-runtime-wry: Windows keeps its native title
// bar, so the space the layout reserves for the traffic lights is dead there.
// Set before the first render so the padding never flashes.
if (navigator.userAgent.includes('Windows')) {
  document.documentElement.classList.add('is-windows');
}

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <DesktopApp />
  </React.StrictMode>
);
