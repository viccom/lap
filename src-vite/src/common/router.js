// src/router.js
import { createRouter, createWebHistory } from 'vue-router';
import { isAuthenticated } from '@/common/auth';

const routes = [
  {
    path: '/',
    name: 'Home',
    component: () => import('@/views/Home.vue'),
  },
  {
    path: '/login',
    name: 'Login',
    component: () => import('@/views/Login.vue'),
  },
  {
    path: '/image-viewer',
    name: 'ImageViewer',
    component: () => import('@/views/ImageViewer.vue'),
  },
  {
    path: '/image-editor',
    name: 'ImageEditor',
    component: () => import('@/views/ImageEditor.vue'),
  },
  {
    path: '/montage',
    name: 'Montage',
    component: () => import('@/views/Montage.vue'),
  },
  {
    path: '/settings',
    name: 'Settings',
    component: () => import('@/views/Settings.vue'),
  },
  {
    path: '/vl-lab',
    name: 'VlLab',
    component: () => import('@/views/VlLab.vue'),
  }
];

const router = createRouter({
  history: createWebHistory(),
  routes,
});

// Web client (phase 4): the whole app sits behind lapd's token. `checkSession`
// resolves before mount in main.js, so the guard sees a settled state. The
// desktop bundle is always authenticated and passes straight through.
router.beforeEach((to) => {
  const isWeb = import.meta.env?.VITE_LAP_TRANSPORT === 'http';
  if (!isWeb) return true;
  if (to.path !== '/login' && !isAuthenticated()) return { path: '/login' };
  if (to.path === '/login' && isAuthenticated()) return { path: '/' };
  return true;
});

export default router;
