// 線画アイコン（16×16 の格子、線幅 1.5）。絵文字や記号フォントは使わない。

const paths: Record<string, string> = {
  select: "M4 2.5l8.5 6-3.8.8 2.1 4.1-1.6.8-2.1-4.1L4 12.6z",
  hand: "M5.5 8.5V3.8a1 1 0 0 1 2 0v4M7.5 7.5V2.8a1 1 0 0 1 2 0v4.7M9.5 7.5V3.8a1 1 0 0 1 2 0v5.4c0 2.8-1.8 4.6-4.3 4.6-1.7 0-2.7-.8-3.6-2.2L2.4 9.3a1 1 0 0 1 1.6-1.1l1.5 1.6",
  text: "M3 3.5h10M8 3.5v9.5M6 13h4",
  rect: "M3 3.5h10v9H3z",
  ellipse: "M8 3a5 5 0 1 0 0 10A5 5 0 0 0 8 3z",
  star: "M8 2.3l1.7 3.6 3.9.5-2.9 2.7.8 3.9L8 11.1 4.5 13l.8-3.9-2.9-2.7 3.9-.5z",
  solid: "M3 3h10v10H3z",
  image: "M2.5 3.5h11v9h-11zM2.5 11l3.2-3.2 2.6 2.4 1.7-1.5 3.5 3M10.5 6.2h.01",
  cube: "M8 2l5.5 3v6L8 14l-5.5-3V5zM2.5 5L8 8l5.5-3M8 8v6",
  null: "M3.5 3.5h9v9h-9zM8 6v4M6 8h4",
  adjust: "M8 2.5a5.5 5.5 0 1 0 0 11zM8 2.5a5.5 5.5 0 0 1 0 11",
  camera: "M2 5h8.5v6H2zM10.5 7l3.5-2v6l-3.5-2",
  light: "M8 2.5a3.5 3.5 0 0 0-2 6.4V11h4V8.9a3.5 3.5 0 0 0-2-6.4zM6.3 13h3.4",
  play: "M5 3.2v9.6L12.8 8z",
  pause: "M4.8 3.2h2.2v9.6H4.8zM9 3.2h2.2v9.6H9z",
  prev: "M11 4v8L5.5 8zM4 4v8",
  next: "M5 4v8l5.5-4zM12 4v8",
  plus: "M8 3v10M3 8h10",
  chevron: "M4.5 6.5L8 10l3.5-3.5",
  twirl: "M6 4.2v7.6L11 8z",
  magnet: "M4 3v5a4 4 0 0 0 8 0V3M2.8 5.5h2.4M10.8 5.5h2.4",
  grid: "M2.5 2.5h11v11h-11zM6.2 2.5v11M9.8 2.5v11M2.5 6.2h11M2.5 9.8h11",
  sidebarL: "M2.5 3h11v10h-11zM6 3v10",
  sidebarR: "M2.5 3h11v10h-11zM10 3v10",
  eye: "M1.8 8S4 3.8 8 3.8 14.2 8 14.2 8 12 12.2 8 12.2 1.8 8 1.8 8zM8 6.2a1.8 1.8 0 1 0 0 3.6 1.8 1.8 0 0 0 0-3.6z",
  eyeOff: "M2.5 2.5l11 11M6.7 6.6a1.8 1.8 0 0 0 2.6 2.6M4.3 4.9C2.8 6 1.8 8 1.8 8S4 12.2 8 12.2c1.3 0 2.4-.4 3.4-1M7 3.9c.3 0 .7-.1 1-.1 4 0 6.2 4.2 6.2 4.2s-.5 1-1.4 2",
  lock: "M4 7.5h8V13H4zM5.5 7.5V5.5a2.5 2.5 0 0 1 5 0v2",
  unlock: "M4 7.5h8V13H4zM5.5 7.5V5.5a2.5 2.5 0 0 1 4.8-1",
  solo: "M8 5a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
  blur: "M10 5a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM2.5 6h3.5M2 8h3.5M2.5 10h3.5",
  diamond: "M8 3.2L12.8 8 8 12.8 3.2 8z",
  close: "M4.5 4.5l7 7M11.5 4.5l-7 7",
  comp: "M2.5 3.5h11v9h-11zM5.5 8a1.3 1.3 0 1 0 0 .01M9 10.5l1.7-3.5 1.8 3.5z",
  footage: "M2.5 3h11v10h-11zM5 3v10M11 3v10M2.5 6h2.5M2.5 10h2.5M11 6h2.5M11 10h2.5",
  audio: "M3 7v2M5.5 5v6M8 3v10M10.5 5.5v5M13 7v2",
  fx: "M3 13l2-10h4M3.8 8h3.7M9 7l4 6M13 7l-4 6",
  more: "M4 8h.01M8 8h.01M12 8h.01",
  fit: "M3 6V3h3M10 3h3v3M13 10v3h-3M6 13H3v-3",
  up: "M8 12.5v-9M4.5 7L8 3.5 11.5 7",
  down: "M8 3.5v9M4.5 9L8 12.5 11.5 9",
  trash: "M3.5 5h9M6.5 5V3.2h3V5M4.8 5l.7 8h5l.7-8",
  threeD: "M8 2l5.5 3v6L8 14l-5.5-3V5zM2.5 5L8 8l5.5-3M8 8v6",
};

export function Icon({ name, size = 16, className }: { name: keyof typeof paths | string; size?: number; className?: string }) {
  const filled = name === "play" || name === "pause" || name === "twirl" || name === "solo";
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill={filled ? "currentColor" : "none"}
      stroke={filled ? "none" : "currentColor"}
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d={paths[name] ?? paths.more} />
    </svg>
  );
}
