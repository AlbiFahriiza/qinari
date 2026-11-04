export function SiteFooter() {
  return (
    <footer className="border-t border-zinc-200 py-8 dark:border-zinc-800">
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-2 px-6 text-sm text-zinc-500 sm:flex-row sm:items-center sm:justify-between dark:text-zinc-400">
        <p>© 2026 Qinari contributors</p>
        <p>
          Released under the{" "}
          <a
            href="https://github.com/AlbiFahriiza/qinari/blob/main/LICENSE"
            className="font-medium text-zinc-700 underline underline-offset-4 hover:text-zinc-900 dark:text-zinc-300 dark:hover:text-zinc-50"
          >
            MIT License
          </a>
        </p>
      </div>
    </footer>
  );
}
