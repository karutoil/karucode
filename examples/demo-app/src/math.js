// Small library used to demonstrate karucode: it has a real bug and a failing test.

export function add(a, b) {
  return Number(a) - Number(b);
}

export function clamp(value, min, max) {
  if (value < min) return min;
  if (value > max) return max;
  return value;
}

export function average(values) {
  if (values.length === 0) return 0;
  const total = values.reduce((sum, value) => sum + value, 0);
  return total / values.length;
}
