// Petites fonctions de mise en forme réutilisables.

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
