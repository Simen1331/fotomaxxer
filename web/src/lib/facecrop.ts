// CSS-cropp av ett ansikt mot 1024-derivatet. bbox er i originalkoordinater;
// vi mapper en lett utvidet bbox-region (litt kontekst rundt) inn på en kvadratisk
// celle via background-size/-position. Ingen ekstra bildegenerering — samme
// derivat som lightboxen bruker.

export function derivedUrl(hash: string, size: number): string {
  return `/derived/${hash}_${size}.jpg`;
}

export function faceCropStyle(
  hash: string,
  bbox: [number, number, number, number],
  width: number | null,
  height: number | null,
  size = 1024,
): React.CSSProperties {
  const url = derivedUrl(hash, size);
  if (!width || !height) {
    return { backgroundImage: `url(${url})`, backgroundSize: "cover", backgroundPosition: "center" };
  }
  const [x, y, w, h] = bbox;
  const pad = 0.4; // vis litt rundt ansiktet, ikke kant-i-kant
  const px = Math.max(0, x - w * pad);
  const py = Math.max(0, y - h * pad);
  const pw = Math.min(width - px, w * (1 + 2 * pad));
  const ph = Math.min(height - py, h * (1 + 2 * pad));
  const sizeX = (width / pw) * 100;
  const sizeY = (height / ph) * 100;
  const posX = pw < width ? (px / (width - pw)) * 100 : 0;
  const posY = ph < height ? (py / (height - ph)) * 100 : 0;
  return {
    backgroundImage: `url(${url})`,
    backgroundSize: `${sizeX}% ${sizeY}%`,
    backgroundPosition: `${posX}% ${posY}%`,
  };
}
