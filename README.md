# Qinari

[![CI](https://github.com/AlbiFahriiza/qinari/actions/workflows/ci.yml/badge.svg)](https://github.com/AlbiFahriiza/qinari/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

A production-grade Next.js starter pack. Strict TypeScript, Tailwind CSS 4,
unit and component tests, formatting, and CI are wired up from the first
commit — clone it and start building the interesting part.

## Tech stack

| Layer      | Choice                                                                        |
| ---------- | ----------------------------------------------------------------------------- |
| Framework  | [Next.js 16](https://nextjs.org) (App Router)                                 |
| Language   | [TypeScript](https://www.typescriptlang.org) (strict)                         |
| UI         | [Tailwind CSS 4](https://tailwindcss.com)                                     |
| Testing    | [Vitest](https://vitest.dev) + [Testing Library](https://testing-library.com) |
| Formatting | [Prettier](https://prettier.io) + tailwindcss class sorting                   |
| CI         | GitHub Actions                                                                |

## Quick start

```bash
git clone https://github.com/AlbiFahriiza/qinari.git
cd qinari
npm install
npm run dev
```

Open [http://localhost:3000](http://localhost:3000). A health endpoint is
available at [`/api/health`](http://localhost:3000/api/health).

## Scripts

| Command                | What it does                      |
| ---------------------- | --------------------------------- |
| `npm run dev`          | Start the development server      |
| `npm run build`        | Production build (Turbopack)      |
| `npm run start`        | Serve the production build        |
| `npm run lint`         | Lint with ESLint (flat config)    |
| `npm run typecheck`    | Type-check with `tsc --noEmit`    |
| `npm run test`         | Run the Vitest suite once         |
| `npm run test:watch`   | Run Vitest in watch mode          |
| `npm run format`       | Format the repo with Prettier     |
| `npm run format:check` | Verify formatting without writing |

## Project structure

```
src/
├── app/                  # App Router pages, layouts, and route handlers
│   ├── api/health/       # JSON health endpoint
│   ├── globals.css       # Tailwind entry and theme tokens
│   ├── layout.tsx        # Root layout with metadata and fonts
│   └── page.tsx          # Landing page
├── components/           # UI components, one concern per file
└── lib/                  # Framework-free helpers, unit-tested
```

Conventions and pipeline details live in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). Contributions are welcome —
start with [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

[MIT](LICENSE)
