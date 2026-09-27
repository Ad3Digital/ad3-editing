/** @inspect text path="Card / Title" */
const title = 'Ajuste direto no projeto';

/** @inspect number min=120 max=800 step=10 path="Card / Width" */
const width = 680;

/** @inspect color path="Card / Accent" */
const accent = '#42dab0';

/** @inspect boolean path="Card / Subtitle" */
const showSubtitle = true;

export const Card = () => <group id="card">
  <rect id="accent" x={48} y={170} width={width} height={8} fill={accent}/>
  <text id="heading" x={48} y={210} fontFamily="Arial" fontSize={38} color="#ffffff">{title}</text>
  {showSubtitle && <text id="subtitle" x={48} y={282} fontFamily="Arial" fontSize={20} color="#acbbc9">
    Variáveis do TSX, controles no editor Rust.
  </text>}
</group>;
