/** Requests per second over the last `windowMs`, from arrival times. */
export function requestRate(arrivals: readonly number[], now: number, windowMs = 10_000): number {
  const n = arrivals.filter((t) => t > now - windowMs && t <= now).length;
  return Math.round((n / (windowMs / 1000)) * 10) / 10;
}

/** Segments lit on the 10-segment meter: one per req/s, at least one if any. */
export function litSegments(rate: number): number {
  return rate <= 0 ? 0 : Math.min(10, Math.ceil(rate));
}
