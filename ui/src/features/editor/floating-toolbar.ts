import { useFloatingToolbar } from '@platejs/floating';
import { useEditorVersion } from 'platejs/react';
import { useLayoutEffect } from 'react';

const doNotMeasureDuringRender = () => {};

/** Keep selection measurement after React has committed the editor DOM. */
export function useCommittedFloatingToolbar(state: Parameters<typeof useFloatingToolbar>[0]) {
  const revision = useEditorVersion();
  const update = state.floating.update;
  // Plate 52 calls update from useEditorSelector. Selectors run during render,
  // when Slate's DOM and the current selection can still belong to the previous
  // commit. Measuring there can also dispatch selection events into React.
  const toolbar = useFloatingToolbar({
    ...state,
    floating: { ...state.floating, update: doNotMeasureDuringRender },
  });
  useLayoutEffect(() => { update(); }, [revision, update]);
  return toolbar;
}
