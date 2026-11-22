// Petites fonctions de mise en forme réutilisables.

/** [80, 443] → « 80, 443 » ; [9850…9853, 9740…9743] → « 9850-9853, 9740-9743 ». */
export function formatPorts(ports: number[]): string {
  const blocs: string[] = [];
  let i = 0;
  while (i < ports.length) {
    let j = i;
    while (j + 1 < ports.length && ports[j + 1] === ports[j] + 1) j++;
    blocs.push(j > i ? `${ports[i]}-${ports[j]}` : `${ports[i]}`);
    i = j + 1;
  }
  return blocs.join(", ");
}

/** Regroupe les identifiants « php-8.5.13#0 » par version : { "8.5.13": [...] }. */
export function groupesPhp<T extends { id: string }>(membres: T[]): [string, T[]][] {
  const m = new Map<string, T[]>();
  for (const x of membres) {
    const v = x.id.replace(/^php-/, "").split("#")[0];
    m.set(v, [...(m.get(v) ?? []), x]);
  }
  return [...m.entries()];
}

/** 34185054 → « 32,6 Mo » (unités françaises : o, Ko, Mo, Go). */
export function formatOctets(n: number): string {
  const unites = ["o", "Ko", "Mo", "Go"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < unites.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toLocaleString("fr-FR", { maximumFractionDigits: i === 0 ? 0 : 1 })} ${unites[i]}`;
}
