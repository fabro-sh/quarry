# Quarry

## Engineering principles

- A promise that crosses a thread/process/network boundary (web worker, WebSocket, RPC) must have something that settles it when the other side dies — an error-event listener, timeout, or heartbeat. `catch` only handles rejections; a dead peer produces silence. Pattern: `rejectOnWorkerError` in `ui/src/features/editor/mirror-serializer.ts`.
