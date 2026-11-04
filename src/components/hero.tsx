export function Hero() {
  return (
    <section className="flex flex-col items-center gap-6 py-24 text-center">
      <p className="rounded-full border border-emerald-600/20 bg-emerald-600/10 px-3 py-1 text-xs font-medium text-emerald-700 dark:border-emerald-400/20 dark:bg-emerald-400/10 dark:text-emerald-400">
        Production-grade starter pack · v0.1.0
      </p>
      <h1 className="max-w-2xl text-5xl font-semibold tracking-tight text-zinc-900 sm:text-6xl dark:text-zinc-50">
        Qinari
      </h1>
      <p className="max-w-xl text-lg leading-8 text-zinc-600 dark:text-zinc-400">
        A production-grade Next.js starter — strict TypeScript, tests,
        formatting, and CI wired up from the first commit, so the tool can start
        with the interesting part.
      </p>
      <div className="flex flex-col gap-3 sm:flex-row">
        <a
          href="#quickstart"
          className="flex h-11 items-center justify-center rounded-lg bg-zinc-900 px-6 text-sm font-medium text-white transition-colors hover:bg-zinc-700 dark:bg-zinc-50 dark:text-zinc-900 dark:hover:bg-zinc-300"
        >
          Quick start
        </a>
        <a
          href="https://github.com/AlbiFahriiza/qinari"
          target="_blank"
          rel="noopener noreferrer"
          className="flex h-11 items-center justify-center rounded-lg border border-zinc-300 px-6 text-sm font-medium text-zinc-700 transition-colors hover:border-zinc-400 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-300 dark:hover:border-zinc-600 dark:hover:bg-zinc-900"
        >
          View on GitHub
        </a>
      </div>
    </section>
  );
}
