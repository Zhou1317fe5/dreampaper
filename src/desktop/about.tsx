import type { ReleaseInfo } from '../types';
import type { DesktopCopy } from './copy';

export type UpdateStatus = 'idle' | 'checking' | 'current' | 'available' | 'failed';

export interface UpdateState {
  status: UpdateStatus;
  info: ReleaseInfo | null;
  error: string | null;
}

const REPOSITORY_URL = 'https://github.com/dream-rec/dreampaper';
const RELEASES_URL = `${REPOSITORY_URL}/releases`;

export function AboutPage({
  d,
  version,
  autoCheck,
  update,
  onAutoCheck,
  onCheck,
  onOpen
}: {
  d: DesktopCopy;
  version: string;
  autoCheck: boolean;
  update: UpdateState;
  onAutoCheck: (enabled: boolean) => void;
  onCheck: () => void;
  onOpen: (url: string) => void;
}) {
  const statusText = (() => {
    switch (update.status) {
      case 'checking':
        return d.about.checking;
      case 'current':
        return d.about.latest;
      case 'available':
        return d.about.available(update.info?.latest_version ?? '');
      case 'failed':
        return update.error ?? d.about.failed;
      default:
        return d.about.idle;
    }
  })();
  const releaseUrl = update.info?.release_url ?? RELEASES_URL;

  return (
    <div className="dp-work single">
      <section className="dp-card about-card">
        <div className="dp-card-body about-body">
          <section className="about-hero">
            <img src="/favor.png" alt="" />
            <div className="about-identity">
              <h2>DreamPaper</h2>
              <p>{d.about.description}</p>
              <small>github.com/dream-rec/dreampaper</small>
            </div>
            <div className="about-project-actions">
              <span className="about-version">v{version}</span>
              <button type="button" className="about-repository" onClick={() => onOpen(REPOSITORY_URL)}>
                <IconGitHub />
                <span>{d.about.openRepository}</span>
              </button>
            </div>
          </section>

          <section className="about-update">
            <div className="about-update-main">
              <span className="about-update-icon"><IconRefresh /></span>
              <div className="about-update-copy">
                <h3>{d.about.update}</h3>
                <div className={`about-update-status ${update.status}`} role="status" aria-live="polite">
                  <span className="about-status-dot" aria-hidden="true" />
                  <span>{statusText}</span>
                </div>
              </div>
            </div>

            <label className="about-auto">
              <input
                type="checkbox"
                checked={autoCheck}
                onChange={(event) => onAutoCheck(event.target.checked)}
              />
              <span>
                <strong>{d.about.autoCheck}</strong>
                <small>{d.about.autoCheckHint}</small>
              </span>
            </label>

            <div className="about-actions">
              <button
                type="button"
                className="dp-primary"
                disabled={update.status === 'checking'}
                onClick={onCheck}
              >
                {update.status === 'checking' ? d.about.checking : d.about.check}
              </button>
              <button type="button" className="dp-ghost" onClick={() => onOpen(releaseUrl)}>
                {update.status === 'available' ? d.about.download : d.about.releases}
              </button>
            </div>
          </section>
        </div>
      </section>
    </div>
  );
}

export function IconGitHub() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 2a10 10 0 0 0-3.16 19.49c.5.09.68-.22.68-.48v-1.87c-2.78.61-3.37-1.18-3.37-1.18-.45-1.16-1.11-1.47-1.11-1.47-.91-.62.07-.61.07-.61 1 .07 1.53 1.03 1.53 1.03.9 1.53 2.35 1.09 2.92.83.09-.65.35-1.09.64-1.34-2.22-.25-4.55-1.11-4.55-4.94 0-1.09.39-1.98 1.03-2.68-.1-.25-.45-1.27.1-2.64 0 0 .84-.27 2.75 1.02A9.6 9.6 0 0 1 12 6.82a9.6 9.6 0 0 1 2.5.34c1.91-1.29 2.75-1.02 2.75-1.02.55 1.37.2 2.39.1 2.64.64.7 1.03 1.59 1.03 2.68 0 3.84-2.34 4.68-4.57 4.93.36.31.68.92.68 1.86v2.76c0 .27.18.58.69.48A10 10 0 0 0 12 2Z" />
    </svg>
  );
}

function IconRefresh() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <path d="M16 6V2.8l-1.5 1.5a6.4 6.4 0 1 0 1.2 7.2" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M16 2.8h-3.2" strokeLinecap="round" />
    </svg>
  );
}
