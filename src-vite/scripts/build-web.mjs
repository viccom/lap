/**
 * Build the Web (lapd-hosted) client: the same Vue pages as the desktop
 * bundle, but with VITE_LAP_TRANSPORT=http so invoke/listen/assetUrl go over
 * the HTTP API instead of Tauri IPC (phase 4).
 *
 * A tiny Node wrapper instead of an inline shell env var so it works on both
 * Windows cmd/PowerShell and POSIX shells without adding cross-env.
 */
import { build } from 'vite';

process.env.VITE_LAP_TRANSPORT = 'http';
await build({ configFile: 'vite.config.js' });
console.log('Web client built into dist/ (VITE_LAP_TRANSPORT=http)');
