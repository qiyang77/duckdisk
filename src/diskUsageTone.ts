export const diskUsageTone = (usedFraction: number) =>
  usedFraction >= 0.85
    ? "critical"
    : usedFraction >= 0.7
    ? "warning"
    : "healthy";
