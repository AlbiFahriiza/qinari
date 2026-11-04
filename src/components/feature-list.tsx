const features = [
  {
    name: "Next.js 16",
    description:
      "App Router with typed routes, React 19, and the Turbopack build pipeline.",
  },
  {
    name: "Strict TypeScript",
    description:
      "End-to-end types checked in CI with tsc --noEmit before anything ships.",
  },
  {
    name: "Tailwind CSS 4",
    description:
      "Utility-first styling with automatic class sorting via the Prettier plugin.",
  },
  {
    name: "Vitest + Testing Library",
    description:
      "Unit and component tests running in jsdom, wired to the same path alias the app uses.",
  },
  {
    name: "GitHub Actions CI",
    description:
      "Typecheck, lint, tests, format check, and production build on every push and pull request.",
  },
  {
    name: "Repo hygiene",
    description:
      "Dependabot updates, issue and pull request templates, MIT license, and contributor docs.",
  },
];

export function FeatureList() {
  return (
    <section className="pb-24">
      <h2 className="text-sm font-medium tracking-widest text-zinc-500 uppercase dark:text-zinc-400">
        What is inside
      </h2>
      <dl className="mt-6 border-t border-zinc-200 dark:border-zinc-800">
        {features.map((feature) => (
          <div
            key={feature.name}
            className="grid gap-1 border-b border-zinc-200 py-4 sm:grid-cols-[220px_1fr] sm:gap-6 dark:border-zinc-800"
          >
            <dt className="text-sm font-semibold text-zinc-900 dark:text-zinc-50">
              {feature.name}
            </dt>
            <dd className="text-sm leading-6 text-zinc-600 dark:text-zinc-400">
              {feature.description}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
