/**
 * 把 Runtime 回報的 UTC 毫秒值顯示成當地時間（spec D-33）。
 * 不指定 `timeZone` 時使用系統時區；指定時區只供測試與診斷使用。
 */
export function formatTimestamp(utcMillis: number, timeZone?: string): string {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  }).formatToParts(new Date(utcMillis));
  const get = (type: Intl.DateTimeFormatPartTypes) => parts.find((part) => part.type === type)!.value;
  return `${get("year")}/${get("month")}/${get("day")} ${get("hour")}:${get("minute")}`;
}
