const BYTE_BASE = 1024;
const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];
const formatters = new Map<number, Intl.NumberFormat>();

const getFormatter = (maximumFractionDigits: number) => {
  let formatter = formatters.get(maximumFractionDigits);
  if (!formatter) {
    formatter = new Intl.NumberFormat(undefined, {
      maximumFractionDigits,
    });
    formatters.set(maximumFractionDigits, formatter);
  }
  return formatter;
};

export const formatBytes = (
  bytes = 0,
  maximumFractionDigits = 1
): string => {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  const index = Math.min(
    Math.floor(Math.log(bytes) / Math.log(BYTE_BASE)),
    UNITS.length - 1
  );
  const value = bytes / Math.pow(BYTE_BASE, index);
  return `${getFormatter(maximumFractionDigits).format(value)} ${
    UNITS[index]
  }`;
};
