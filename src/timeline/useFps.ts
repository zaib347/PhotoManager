import { useEffect, useState } from "react";

/** Frames rendered in the last second, for the M0 performance overlay. */
export function useFps() {
  const [fps, setFps] = useState(0);
  useEffect(() => {
    let frames = 0;
    let windowStart = performance.now();
    let handle = requestAnimationFrame(function tick(now) {
      frames++;
      if (now - windowStart >= 1000) {
        setFps(Math.round((frames * 1000) / (now - windowStart)));
        frames = 0;
        windowStart = now;
      }
      handle = requestAnimationFrame(tick);
    });
    return () => cancelAnimationFrame(handle);
  }, []);
  return fps;
}
