import { Link } from 'react-router-dom';

/**
 * Back-to-home affordance: a react-router link to the trip list ("/") rendered
 * as a small pill (chevron + "Trips"). The `frosted` variant is dark glass for
 * use over photos (e.g. the trip hero); the default quiet variant is a bordered
 * surface chip for plain backgrounds (e.g. the review queue).
 */
export function BackHome({ frosted = false }: { frosted?: boolean }) {
  return (
    <Link to="/" className={`back-home${frosted ? ' frosted' : ''}`} aria-label="Back to all trips">
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden
      >
        <path d="M15 18l-6-6 6-6" />
      </svg>
      Trips
    </Link>
  );
}
