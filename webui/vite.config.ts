import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// base: './' 让产物以相对路径引用资源，便于被 rust-embed 嵌入后从根路径 '/' 托管。
export default defineConfig({
  plugins: [vue()],
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
})
