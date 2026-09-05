
const P = (d) => `<path d="${d}"/>`;

const ICONS = {
  gauge: P("M2.2 12.6a6.3 6.3 0 1 1 11.6 0") + P("M8 12.4 10.6 8.6") + `<circle cx="8" cy="12.4" r="1" fill="currentColor" stroke="none"/>`,

  pen: P("M2.7 13.3l.8-3 7.3-7.3a1.7 1.7 0 0 1 2.4 2.4l-7.3 7.3z") + P("M9.6 4.2l2.2 2.2"),

  export: P("M8 9.5V2.5") + P("M5.2 5 8 2.2 10.8 5") + P("M2.8 10.2v1.6a1.7 1.7 0 0 0 1.7 1.7h7a1.7 1.7 0 0 0 1.7-1.7v-1.6"),

  x: P("M4 4l8 8") + P("M12 4l-8 8"),

  swap: P("M2.6 5.4h9.6") + P("M9.8 3 12.2 5.4 9.8 7.8") + P("M13.4 10.6H3.8") + P("M6.2 8.2 3.8 10.6l2.4 2.4"),

  power: P("M8 2.2v5.6") + P("M4.9 4.4a4.9 4.9 0 1 0 6.2 0"),

  sliders: P("M2.2 4.8h5") + P("M10.6 4.8h3.2") + P("M2.2 11.2h3.2") + P("M8.8 11.2h5") + `<circle cx="8.9" cy="4.8" r="1.7"/>` + `<circle cx="7.1" cy="11.2" r="1.7"/>`,

  lock: `<rect x="3.4" y="7" width="9.2" height="6.6" rx="1.6"/>` + P("M5.5 7V5.2a2.5 2.5 0 0 1 5 0V7") + P("M8 9.6v1.8"),

  lockOpen: `<rect x="3.4" y="7" width="9.2" height="6.6" rx="1.6"/>` + P("M5.5 7V5.2a2.5 2.5 0 0 1 4.9-.6") + P("M8 9.6v1.8"),

  check: P("M3 8.5l3.4 3.3L13 4.8"),

  alert: P("M8 2.3 14.7 13.7H1.3z") + P("M8 6.6v3") + `<circle cx="8" cy="11.5" r=".7" fill="currentColor" stroke="none"/>`,

  chev: P("M6 3.5 10.5 8 6 12.5"),
};

/* 24 系图标（外部图标库，如 Lucide），stroke 视觉粗细与 16 系对齐 */
const ICONS24 = {
  bot: `<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>`,
};

export function ic(name, size = 16, cls = "") {
  const body24 = ICONS24[name];
  if (body24) {
    return `<svg class="ic${cls ? " " + cls : ""}" width="${size}" height="${size}" viewBox="0 0 24 24"
      fill="none" stroke="currentColor" stroke-width="1.7"
      stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body24}</svg>`;
  }
  const body = ICONS[name];
  if (!body) return "";
  return `<svg class="ic${cls ? " " + cls : ""}" width="${size}" height="${size}" viewBox="0 0 16 16"
    fill="none" stroke="currentColor" stroke-width="1.5"
    stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
}
