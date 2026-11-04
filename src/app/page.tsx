import { FeatureList } from "@/components/feature-list";
import { Hero } from "@/components/hero";
import { Quickstart } from "@/components/quickstart";
import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";

export default function Home() {
  return (
    <div className="flex min-h-dvh flex-col bg-zinc-50 font-sans text-zinc-900 dark:bg-zinc-950 dark:text-zinc-50">
      <SiteHeader />
      <main className="mx-auto w-full max-w-5xl flex-1 px-6">
        <Hero />
        <Quickstart />
        <FeatureList />
      </main>
      <SiteFooter />
    </div>
  );
}
