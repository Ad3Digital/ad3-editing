import { Match, Switch } from 'solid-js';
import { useTicker } from '@ad3-editing/jsx';

// Seek forward/backward to exercise priority, fallback and live Match values.
export default () => {
  const ticker = useTicker();
  return (
    <stage background="#171717">
      <scene id="main" active width={640} height={360} workarea={[0, 6]}>
        <Switch fallback={<rect id="idle" width={640} height={360} fill="#334155" />}>
          <Match when={ticker.time() >= 3 && ticker.time()}>
            {time => <>
              <rect id="late" width={640} height={360} fill="#15803d" />
              <text id="late-label" x={80} y={140} fontSize={36} fill="#ffffff">
                First match: {time().toFixed(1)}s
              </text>
            </>}
          </Match>
          <Match when={ticker.time() >= 1 && ticker.time()}>
            {time => <>
              <rect id="early" width={640} height={360} fill="#1d4ed8" />
              <text id="early-label" x={80} y={140} fontSize={36} fill="#ffffff">
                Second match: {time().toFixed(1)}s
              </text>
            </>}
          </Match>
        </Switch>
      </scene>
    </stage>
  );
};
