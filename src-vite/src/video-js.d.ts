// video.js ships without type declarations for these entry points. The app
// touches them through the documented API surface only; keep this shim minimal
// rather than pulling @types/video.js (which lags major versions).
declare module 'video.js/core';
declare module 'video.js/dist/video-js.min.css';
