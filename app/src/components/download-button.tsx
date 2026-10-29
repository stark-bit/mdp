export function DownloadButton({ onDownload }: { onDownload: () => void }) {
  return (
    <button
      type="button"
      className="download-button"
      onClick={onDownload}
      aria-label="Download HTML"
      title="Download HTML"
      contentEditable={false}
    >
      <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
        <path d="M8.75 1.75a.75.75 0 0 0-1.5 0v6.69L4.53 5.72a.75.75 0 0 0-1.06 1.06l4 4a.75.75 0 0 0 1.06 0l4-4a.75.75 0 1 0-1.06-1.06L8.75 8.44V1.75ZM2.5 11a.75.75 0 0 0-1.5 0v2.5c0 .83.67 1.5 1.5 1.5h11c.83 0 1.5-.67 1.5-1.5V11a.75.75 0 0 0-1.5 0v2.5h-11V11Z" />
      </svg>
    </button>
  );
}
