import { For, Show, createSignal, onMount } from 'solid-js';
import { useTicker } from '@ad3-editing/jsx';

const bars = [
  { id: 'one', color: '#4ce1b0', phase: 0 },
  { id: 'two', color: '#5b8cff', phase: 1.3 },
  { id: 'three', color: '#f6ad55', phase: 2.6 },
];

export default function Project() {
  const ticker = useTicker();
  const [ready, setReady] = createSignal(false);
  onMount(() => setReady(true));

  return <stage>
    <scene id="main" active width={960} height={540} workarea={[0, 6]} fill="#101720">
      <text id="eyebrow" x={48} y={44} fontFamily="Arial" fontSize={18} color="#4ce1b0" end={6}>AD3 EDITING / RUST</text>
      <text id="title" x={48} y={82} fontFamily="Arial" fontSize={44} color="#ffffff" end={6}>Uma montagem. Frames vivos.</text>
      <text id="clock" x={48} y={151} fontFamily="Arial" fontSize={22} color="#aeb9c7" end={6}>
        {`Frame ${ticker.frame().toString().padStart(3, '0')}  /  ${ticker.time().toFixed(2)} s`}
      </text>
      <For each={bars}>{(bar, index) =>
        <rect id={bar.id} x={48 + (Math.sin(ticker.time() * 1.8 + bar.phase) + 1) * 190}
          y={220 + index() * 68} width={360} height={42} fill={bar.color} end={6} />
      }</For>
      <Show when={ready()}>
        <text id="ready" x={48} y={463} fontFamily="Arial" fontSize={18} color="#aeb9c7" end={6}>
          Sinais, efeitos, listas e relogio executados no host Rust.
        </text>
      </Show>
    </scene>
  </stage>;
}
