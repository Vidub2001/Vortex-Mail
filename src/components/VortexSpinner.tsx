// Indicateur d'activité de Vortex Mail (synchronisation, recherche,
// chargement) : un petit tourbillon de trois arcs qui tournent à des vitesses
// différentes, à la place de l'anneau générique de Fluent UI. Couleur
// d'accent du thème ; sans rotation si le système demande moins d'animations
// (styles.css, `.vortex-spinner`).
import type { CSSProperties } from "react";

interface Props {
  /** Taille du dessin en pixels (16 par défaut, comme les icônes de la barre d'état). */
  size?: number;
  /** Texte affiché à côté ; sans lui, `aria-label` nomme l'attente pour les lecteurs d'écran. */
  label?: string;
  "aria-label"?: string;
  className?: string;
}

export function VortexSpinner({ size = 16, label, className, ...rest }: Props) {
  const style = { "--vortex-size": `${size}px` } as CSSProperties;
  return (
    <span
      className={className ? `vortex-spinner ${className}` : "vortex-spinner"}
      role="status"
      aria-label={label ? undefined : rest["aria-label"]}
      style={style}
    >
      <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <circle className="vortex-arc vortex-arc-outer" cx="12" cy="12" r="10" pathLength="100" />
        <circle className="vortex-arc vortex-arc-middle" cx="12" cy="12" r="6.5" pathLength="100" />
        <circle className="vortex-arc vortex-arc-inner" cx="12" cy="12" r="3" pathLength="100" />
      </svg>
      {label && <span className="vortex-spinner-label">{label}</span>}
    </span>
  );
}
