// Thèmes Vortex Mail : bleu du logo, version sombre (maquette) et claire.
import { createDarkTheme, createLightTheme, type BrandVariants, type Theme } from "@fluentui/react-components";
import { useEffect, useState } from "react";

/** Nuancier construit autour du bleu du logo (#1e88e5, rang 80). */
const vortex: BrandVariants = {
  10: "#030812",
  20: "#061a30",
  30: "#0a2c4e",
  40: "#0e3f6c",
  50: "#12518b",
  60: "#1663a9",
  70: "#1a76c7",
  80: "#1e88e5",
  90: "#3896e8",
  100: "#52a3ec",
  110: "#6cb1ef",
  120: "#86bef2",
  130: "#9fccf5",
  140: "#b9daf8",
  150: "#d3e7fc",
  160: "#edf5ff",
};

export const lightTheme: Theme = {
  ...createLightTheme(vortex),
  colorNeutralBackground1: "#ffffff",
  colorNeutralBackground2: "#f6f7f9",
  colorNeutralBackground3: "#eceff3",
  colorNeutralBackground1Hover: "#f3f5f8",
  colorNeutralBackground1Selected: "#e7f1fc",
  colorSubtleBackgroundHover: "#eaedf1",
  colorNeutralStroke2: "#e1e4e8",
  colorNeutralStroke3: "#eceef1",
};

/** Sombre façon maquette : gris anthracite, accents bleus. */
export const darkTheme: Theme = {
  ...createDarkTheme(vortex),
  colorBrandForeground1: vortex[110],
  colorBrandForeground2: vortex[120],
  colorBrandForegroundLink: vortex[110],
  colorNeutralBackground1: "#1d1e22",
  colorNeutralBackground2: "#18191c",
  colorNeutralBackground3: "#121316",
  colorNeutralBackground1Hover: "#25272c",
  colorNeutralBackground1Selected: "#262b34",
  colorSubtleBackgroundHover: "#2a2c31",
  colorSubtleBackgroundPressed: "#303238",
  colorNeutralStroke2: "#2c2e33",
  colorNeutralStroke3: "#25272b",
  colorNeutralForeground2: "#c5c7cc",
  colorNeutralForeground3: "#9a9da4",
};

export type ThemeMode = "system" | "light" | "dark";

const KEY = "vortex.theme";

function systemDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** Choix du thème (mémorisé) et thème effectif, qui suit le système en mode « system ». */
export function useThemeMode(): { mode: ThemeMode; setMode: (m: ThemeMode) => void; dark: boolean } {
  const [mode, setModeState] = useState<ThemeMode>(() => {
    try {
      const saved = localStorage.getItem(KEY);
      return saved === "light" || saved === "dark" ? saved : "system";
    } catch {
      return "system";
    }
  });
  const [sysDark, setSysDark] = useState(systemDark);
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent) => setSysDark(e.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  const setMode = (m: ThemeMode) => {
    setModeState(m);
    try {
      localStorage.setItem(KEY, m);
    } catch {
      // choix valable pour cette session
    }
  };
  const dark = mode === "dark" || (mode === "system" && sysDark);
  return { mode, setMode, dark };
}
