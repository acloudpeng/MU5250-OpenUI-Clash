import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  base: './',
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          // The QR decoder loads only when the eSIM page reads an image.
          if (id.includes('node_modules') && !id.includes('/jsqr/')) return 'vendor'
        },
      },
    },
  },
})
