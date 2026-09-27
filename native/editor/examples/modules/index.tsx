import { Title } from './title';
import palette from './palette.json';

const bars = Array.from({ length: 8 }, (_, index) => ({
  id: `bar-${index}`,
  x: 42 + index * 71,
  height: 32 + Math.round(Math.sin(index * 0.55 + 0.2) ** 2 * 100),
}));

export default function Project() {
  return (
    <stage>
      <scene id="modules" active name="Native modules" width={640} height={360}
        fill={palette.background} workarea={[0, 3]}>
        <Title color={palette.text} />
        {bars.map(bar => (
          <rect id={bar.id} x={bar.x} y={302 - bar.height}
            width={48} height={bar.height} fill={palette.accent} end={3} />
        ))}
      </scene>
    </stage>
  );
}
