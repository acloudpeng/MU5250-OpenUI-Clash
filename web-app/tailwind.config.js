/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ['class', '[data-theme="dark"]'],
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  theme: {
    extend: {
      colors: {
        bg: 'rgb(var(--bg) / <alpha-value>)',
        surface: 'rgb(var(--surface) / <alpha-value>)',
        surface2: 'rgb(var(--surface-2) / <alpha-value>)',
        raised: 'rgb(var(--raised) / <alpha-value>)',
        line: 'rgb(var(--line) / <alpha-value>)',
        ink: 'rgb(var(--text) / <alpha-value>)',
        ink2: 'rgb(var(--text-2) / <alpha-value>)',
        ink3: 'rgb(var(--text-3) / <alpha-value>)',
        accent: 'rgb(var(--accent) / <alpha-value>)',
        ok: 'rgb(var(--ok) / <alpha-value>)',
        warn: 'rgb(var(--warn) / <alpha-value>)',
        danger: 'rgb(var(--danger) / <alpha-value>)',
      },
      borderRadius: {
        '2xl': '16px',
        xl: '12px',
        lg: '10px',
      },
      // Hairlines and tints: border-line/8, bg-danger/8 and the like.
      opacity: {
        6: '0.06',
        8: '0.08',
        12: '0.12',
      },
      boxShadow: {
        card: '0 1px 2px rgb(var(--shadow) / 0.04), 0 1px 3px rgb(var(--shadow) / 0.06)',
        pop: '0 12px 32px rgb(var(--shadow) / 0.18), 0 2px 6px rgb(var(--shadow) / 0.08)',
      },
      maxWidth: {
        shell: '72rem',
      },
    },
  },
  plugins: [],
}
