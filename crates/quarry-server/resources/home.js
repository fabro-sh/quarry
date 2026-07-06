function copyWithExecCommand(text) {
  const scratch = document.createElement('textarea');
  scratch.value = text;
  scratch.setAttribute('readonly', '');
  scratch.style.position = 'fixed';
  scratch.style.opacity = '0';
  document.body.appendChild(scratch);
  scratch.select();
  const copied = document.execCommand('copy');
  scratch.remove();
  return copied;
}

for (const button of document.querySelectorAll('button[data-copy]')) {
  button.addEventListener('click', async () => {
    const source = document.getElementById(button.dataset.copy);
    if (!source) return;
    const text = source.textContent.trim();
    let copied = false;
    if (navigator.clipboard) {
      copied = await navigator.clipboard.writeText(text).then(
        () => true,
        () => false
      );
    }
    if (!copied) copied = copyWithExecCommand(text);
    button.textContent = copied ? 'Copied' : 'Copy failed';
    setTimeout(() => {
      button.textContent = 'Copy';
    }, 2000);
  });
}
