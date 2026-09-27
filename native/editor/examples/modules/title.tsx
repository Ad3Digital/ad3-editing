type Props = { color: string };

export function Title({ color }: Props) {
  return (
    <>
      <text id="heading" x={40} y={32} fontFamily="Arial"
        fontSize={42} color={color} end={3}>AD3 Editing · Rust</text>
      <text id="subtitle" x={42} y={102} fontFamily="Arial"
        fontSize={18} color={color} end={3}>Componentes TSX + módulos locais</text>
    </>
  );
}
