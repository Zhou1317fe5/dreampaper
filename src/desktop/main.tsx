import React from 'react';
import { createRoot } from 'react-dom/client';
import { DesktopApp } from './shell';
import '../styles.css';
import './styles.css';

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <DesktopApp />
  </React.StrictMode>
);
