import { useWorld } from "@ad3-editing/koota-solid";
import { getRuntimeDocument } from "@ad3-editing/reconciler";

export function useDocument() {
  const world = useWorld();
  return () => getRuntimeDocument(world);
}
