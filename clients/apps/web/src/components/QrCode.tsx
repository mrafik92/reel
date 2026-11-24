import { createMemo, type JSX } from "solid-js";
import qrcode from "qrcode-generator";

/** A QR code as inline SVG: dark modules on a white plate, so phones can read it in any theme. */
export function QrCode(props: { value: string; label: string; size?: number }): JSX.Element {
  const code = createMemo(() => {
    const qr = qrcode(0, "M");
    qr.addData(props.value);
    qr.make();
    const n = qr.getModuleCount();
    let d = "";
    for (let y = 0; y < n; y++) {
      for (let x = 0; x < n; x++) if (qr.isDark(y, x)) d += `M${x} ${y}h1v1h-1z`;
    }
    return { n, d };
  });
  const quiet = 4; // the margin the QR spec asks for
  return (
    <svg
      class="qr"
      role="img"
      aria-label={props.label}
      width={props.size ?? 208}
      height={props.size ?? 208}
      viewBox={`${-quiet} ${-quiet} ${code().n + 2 * quiet} ${code().n + 2 * quiet}`}
      shape-rendering="crispEdges"
    >
      <rect x={-quiet} y={-quiet} width={code().n + 2 * quiet} height={code().n + 2 * quiet} fill="#fff" />
      <path d={code().d} fill="#000" />
    </svg>
  );
}
