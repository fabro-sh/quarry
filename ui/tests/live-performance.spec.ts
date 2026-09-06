import { expect, test, type Page } from 'playwright/test';
import { writeFileSync } from 'node:fs';
import { blocks, body, createDocument, openDocument, select, transaction } from './helpers/native-document';

// Trace snapshots traverse the full DOM during input and can themselves block
// a frame. Measure the app without that work; retain JSON metrics and optional
// CPU profiles here. The conformance suite still records failure traces.
test.use({ trace: 'off' });

test('a 100 KiB document stays responsive during typing and reloads the exact saved text', async ({ browser, browserName, request }) => {
  test.setTimeout(120000);
  const markdown = Array.from({ length: 800 }, (_, n) => `${n}: Review TARGET with Unicode 😀 and **formatted text**. ${'Detail '.repeat(12)}`).join('\n\n');
  expect(Buffer.byteLength(markdown)).toBeGreaterThan(100 * 1024);
  const started = Date.now();
  const fixture = await createDocument(request, markdown);
  const initial = await blocks(request, fixture.url); const target = initial[400];
  const user = await openDocument(browser, fixture.path, 'Writer');
  const opened = Date.now() - started;
  try {
    await profileNative(user.page);
    await select(user.page, target.block_id, 0);
    const paragraph = body(user.page).locator(`[data-block-id="${target.block_id}"]`);
    const samples: number[] = [];
    for (let n = 1; n <= 20; n++) {
      const start = Date.now(); await user.page.keyboard.type('x');
      await expect(paragraph).toHaveText('x'.repeat(n) + target.text);
      samples.push(Date.now() - start);
    }
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved', { timeout: 60000 });
    await expect.poll(async () => (await blocks(request, fixture.url))[400].text).toBe('x'.repeat(20) + target.text);
    const nativeProfile = process.env.QUARRY_PROFILE ? await user.page.evaluate(() => (window as unknown as { nativeProfile: unknown }).nativeProfile) : undefined;
    const start = Date.now(); await user.page.reload(); await expect(paragraph).toHaveText('x'.repeat(20) + target.text);
    const reload = Date.now() - start;
    samples.sort((a, b) => a - b);
    const metrics = { nativeProfile, browser: browserName, markdown_bytes: Buffer.byteLength(markdown), blocks: initial.length,
      open_ms: opened, keystroke_ms: { p50: samples[10], p95: samples[18], max: samples[19] }, reload_ms: reload };
    console.info('QUARRY_PERFORMANCE_METRICS', JSON.stringify(metrics));
    await test.info().attach('document-performance.json', { body: JSON.stringify(metrics, null, 2), contentType: 'application/json' });
    // End-to-end budgets include browser automation, rendering and queued saves.
    expect(metrics.keystroke_ms.p95, JSON.stringify(metrics)).toBeLessThan(250);
    expect(metrics.keystroke_ms.max, JSON.stringify(metrics)).toBeLessThan(750);
    expect(reload, JSON.stringify(metrics)).toBeLessThan(10000);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});


for (const blockIndex of [0, 700, 1400]) test(`Chrome retains rapid typing responsiveness in the 100 KiB benchmark at block ${blockIndex}`, async ({ browser, browserName, request }) => {
  test.skip(browserName !== 'chromium', 'Direct comparison with the previous Chrome benchmark');
  test.setTimeout(180000);
  const largeBody = Array.from({ length: 1400 }, (_, index) => `Paragraph ${index.toString().padStart(4, '0')}: ${'content '.repeat(8)}`).join('\n\n');
  const markdown = `# Performance\n\n${largeBody}\n`;
  const fixture = await createDocument(request, markdown);
  const target = (await blocks(request, fixture.url))[blockIndex];
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    let publications = 0;
    user.page.on('request', (request) => { if (request.method() === 'POST' && request.url().includes('/document-commands')) publications++; });
    await profileNative(user.page);
    const profiler = process.env.QUARRY_CPU_PROFILE ? await user.page.context().newCDPSession(user.page) : undefined;
    if (profiler) { await profiler.send('Profiler.enable'); await profiler.send('Profiler.start'); }
    const paragraph = body(user.page).locator(`[data-block-id="${target.block_id}"]`);
    await paragraph.scrollIntoViewIfNeeded();
    await select(user.page, target.block_id, 0);
    await user.page.evaluate(() => {
      const metrics = { frameGaps: [] as number[], keyToFrame: [] as number[], running: true };
      (window as unknown as { typingMetrics: typeof metrics }).typingMetrics = metrics;
      let previous = performance.now();
      const frame = (now: number) => { metrics.frameGaps.push(now - previous); previous = now; if (metrics.running) requestAnimationFrame(frame); };
      requestAnimationFrame(frame);
      document.addEventListener('keydown', () => { const start = performance.now(); requestAnimationFrame(() => metrics.keyToFrame.push(performance.now() - start)); }, { capture: true });
    });
    const typed = 'abcdefghij'.repeat(10); const typingStarted = Date.now(); await user.page.keyboard.type(typed);
    await expect(paragraph).toHaveText(typed + target.text);
    const metrics = await user.page.evaluate(async () => {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      const metrics = (window as unknown as { typingMetrics: { frameGaps: number[]; keyToFrame: number[]; running: boolean } }).typingMetrics;
      metrics.running = false; return metrics;
    });
    if (profiler) { const { profile } = await profiler.send('Profiler.stop'); await saveCpuProfile(user.page, profile); await profiler.detach(); }
    if (process.env.QUARRY_RENDER_PROFILE) await saveRenderProfile(user.page, process.env.QUARRY_RENDER_PROFILE);
    const sorted = [...metrics.keyToFrame].sort((a, b) => a - b);
    const result = { browser_version: browser.version(), nativeProfile: process.env.QUARRY_PROFILE ? await user.page.evaluate(() => (window as unknown as { nativeProfile: unknown }).nativeProfile) : undefined, markdown_bytes: Buffer.byteLength(markdown), block_index: blockIndex, keys: typed.length, p50_key_to_frame_ms: sorted[50], p95_key_to_frame_ms: sorted[94], max_frame_gap_ms: Math.max(...metrics.frameGaps) };
    console.info('QUARRY_CHROME_CURRENT', JSON.stringify(result));
    await test.info().attach('chrome-typing.json', { body: JSON.stringify({ ...result, frame_gaps: metrics.frameGaps, key_to_frame: metrics.keyToFrame }, null, 2), contentType: 'application/json' });
    expect(metrics.keyToFrame).toHaveLength(typed.length);
    // One-frame Chrome responsiveness is a product requirement.
    expect(result.p95_key_to_frame_ms, JSON.stringify(result)).toBeLessThanOrEqual(20);
    expect(result.max_frame_gap_ms, JSON.stringify(result)).toBeLessThanOrEqual(50);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved', { timeout: 120000 });
    const publication = { keys: typed.length, requests: publications, typing_and_save_ms: Date.now() - typingStarted };
    console.info('QUARRY_CHROME_PUBLICATION', JSON.stringify(publication));
    await test.info().attach('chrome-publication.json', { body: JSON.stringify(publication, null, 2), contentType: 'application/json' });
    expect(publication.requests).toBeLessThan(25);
    expect(publication.typing_and_save_ms).toBeLessThan(15000);
    await user.page.reload(); await expect(paragraph).toHaveText(typed + target.text);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

async function profileNative(page: Page) {
    if (process.env.QUARRY_CPU_PROFILE || process.env.QUARRY_RENDER_PROFILE) await page.evaluate(() => {
      const frames: unknown[] = [];
      (window as unknown as { renderProfile: unknown[] }).renderProfile = frames;
      if (PerformanceObserver.supportedEntryTypes.includes('long-animation-frame')) {
        new PerformanceObserver((list) => frames.push(...list.getEntries().map((entry) => entry.toJSON())))
          .observe({ type: 'long-animation-frame' });
      }
    });
    if (process.env.QUARRY_PROFILE) await page.evaluate(async () => {
      const path = '/src/generated/document/quarry_document.js';
      const { NativeDocument, NativeCommandBuilder } = await import(path);
      const profile: Record<string, { calls: number; ms: number; max: number }> = {};
      (window as unknown as { nativeProfile: unknown }).nativeProfile = profile;
      for (const [prefix, type, names] of [
        ['', NativeDocument, ['apply', 'view', 'save', 'save_after', 'point', 'selection', 'locate_point', 'merge', 'merge_changes', 'contains_history', 'fork', 'heads', 'command_builder']],
        ['builder.', NativeCommandBuilder, ['push', 'view', 'block_view', 'point', 'selection', 'finish']],
      ] as const) for (const name of names) {
        const original = type.prototype[name];
        type.prototype[name] = function (...args: unknown[]) {
          const start = performance.now(); try { return original.apply(this, args); }
          finally { const ms = performance.now() - start; const count = profile[prefix + name] ??= { calls: 0, ms: 0, max: 0 }; count.calls++; count.ms += ms; count.max = Math.max(count.max, ms); }
        };
      }
    });
}

async function saveCpuProfile(page: Page, profile: unknown) {
  const file = profileFile(process.env.QUARRY_CPU_PROFILE!);
  writeFileSync(file, JSON.stringify(profile));
  await saveRenderProfile(page, process.env.QUARRY_CPU_PROFILE!);
}

function profileFile(prefix: string) {
  const info = test.info();
  return `${prefix}.${info.testId.replace(/[^a-zA-Z0-9_-]/g, '-')}.repeat-${info.repeatEachIndex}`;
}

async function saveRenderProfile(page: Page, prefix: string) {
  const file = profileFile(prefix);
  const frames = await page.evaluate(() => (window as unknown as { renderProfile: unknown[] }).renderProfile);
  writeFileSync(file + '.frames.json', JSON.stringify(frames, null, 2));
  console.info('QUARRY_RENDER_PROFILE', JSON.stringify({ file: file + '.frames.json', frames }));
}

for (const blockIndex of [0, 1400]) test(`Chrome stays responsive when an agent review arrives during sustained typing at block ${blockIndex}`, async ({ browser, browserName, request }) => {
  test.skip(browserName !== 'chromium', 'Chrome input latency requirement');
  test.setTimeout(180000);
  const markdown = '# Performance\n\n' + Array.from({ length: 1400 }, (_, n) => `Paragraph ${n}: ${'content '.repeat(8)}`).join('\n\n');
  const fixture = await createDocument(request, markdown);
  const initial = await blocks(request, fixture.url), target = initial[blockIndex];
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await profileNative(user.page);
    const profiler = process.env.QUARRY_CPU_PROFILE ? await user.page.context().newCDPSession(user.page) : undefined;
    if (profiler) { await profiler.send('Profiler.enable'); await profiler.send('Profiler.start'); }
    const paragraph = body(user.page).locator(`[data-block-id="${target.block_id}"]`);
    await paragraph.scrollIntoViewIfNeeded();
    await select(user.page, target.block_id, 0);
    await user.page.evaluate(() => {
      const metrics = { gaps: [] as number[], keys: [] as number[], count: 0, remoteAt: 0, running: true };
      (window as unknown as { concurrentMetrics: typeof metrics }).concurrentMetrics = metrics;
      let previous = performance.now();
      const frame = (now: number) => { metrics.gaps.push(now - previous); previous = now; if (metrics.running) requestAnimationFrame(frame); };
      requestAnimationFrame(frame);
      document.addEventListener('keydown', () => { metrics.count++; const start = performance.now(); requestAnimationFrame(() => metrics.keys.push(performance.now() - start)); }, { capture: true });
    });
    const typed = 'abcdefghij'.repeat(40);
    await Promise.all([
      user.page.keyboard.type(typed, { delay: 8 }),
      (async () => {
        await user.page.waitForFunction(() => (window as unknown as { concurrentMetrics: { count: number } }).concurrentMetrics.count >= 10);
        await transaction(request, fixture.url, [
          { op: 'comment.add', block_id: initial[700].block_id, start: 0, end: 9, body: 'Concurrent large review' },
          { op: 'replace_block_content', block_id: initial[700].block_id, text: 'Agent ' + initial[700].text },
        ], initial[700].document_clock);
        await expect(body(user.page).locator('[data-comment-id]')).toHaveText('Paragraph');
        await user.page.evaluate(() => { const metrics = (window as unknown as { concurrentMetrics: { count: number; remoteAt: number } }).concurrentMetrics; metrics.remoteAt = metrics.count; });
      })(),
    ]);
    const measured = await user.page.evaluate(async () => {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      const metrics = (window as unknown as { concurrentMetrics: { keys: number[]; gaps: number[]; running: boolean; remoteAt: number } }).concurrentMetrics;
      metrics.running = false; const sorted = [...metrics.keys].sort((a, b) => a - b);
      return { p95_key_to_frame_ms: sorted[Math.floor(sorted.length * .95) - 1], max_frame_gap_ms: Math.max(...metrics.gaps), keys: metrics.keys.length, remote_at_key: metrics.remoteAt,
        max_frame_index: metrics.gaps.indexOf(Math.max(...metrics.gaps)), frame_gaps: metrics.gaps, key_to_frame: metrics.keys };
    });
    if (profiler) { const { profile } = await profiler.send('Profiler.stop'); await saveCpuProfile(user.page, profile); await profiler.detach(); }
    if (process.env.QUARRY_RENDER_PROFILE) await saveRenderProfile(user.page, process.env.QUARRY_RENDER_PROFILE);
    const { frame_gaps: _gaps, key_to_frame: _keys, ...summary } = measured;
    console.info('QUARRY_CHROME_CONCURRENT', JSON.stringify({ browser_version: browser.version(), block_index: blockIndex, ...summary, nativeProfile: process.env.QUARRY_PROFILE ? await user.page.evaluate(() => (window as unknown as { nativeProfile: unknown }).nativeProfile) : undefined }));
    await test.info().attach('chrome-concurrent.json', { body: JSON.stringify({ browser_version: browser.version(), ...measured }, null, 2), contentType: 'application/json' });
    expect(measured.keys).toBe(typed.length); expect(measured.remote_at_key).toBeLessThan(typed.length);
    expect(measured.p95_key_to_frame_ms).toBeLessThanOrEqual(20); expect(measured.max_frame_gap_ms).toBeLessThanOrEqual(50);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved', { timeout: 120000 });
    await user.page.reload(); await expect(paragraph).toHaveText(typed + target.text);
    await expect(body(user.page).locator(`[data-block-id="${initial[700].block_id}"]`)).toHaveText('Agent ' + initial[700].text);
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('Paragraph'); expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
