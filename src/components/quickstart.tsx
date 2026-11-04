const commands = ["npm install", "npm run dev"];

export function Quickstart() {
  return (
    <section id="quickstart" className="scroll-mt-8 pb-20">
      <div className="rounded-xl border border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900">
        <div className="flex items-center gap-1.5 border-b border-zinc-200 px-4 py-3 dark:border-zinc-800">
          <span className="h-2.5 w-2.5 rounded-full bg-zinc-300 dark:bg-zinc-700" />
          <span className="h-2.5 w-2.5 rounded-full bg-zinc-300 dark:bg-zinc-700" />
          <span className="h-2.5 w-2.5 rounded-full bg-zinc-300 dark:bg-zinc-700" />
          <span className="ml-2 text-xs font-medium text-zinc-500 dark:text-zinc-400">
            terminal
          </span>
        </div>
        <pre className="overflow-x-auto px-4 py-4 font-mono text-sm leading-7 text-zinc-800 dark:text-zinc-200">
          {commands.map((command) => (
            <code key={command} className="block">
              <span className="text-emerald-600 select-none dark:text-emerald-400">
                ${" "}
              </span>
              {command}
            </code>
          ))}
        </pre>
      </div>
    </section>
  );
}
