interface BrandMarkProps {
  size?: number;
  className?: string;
}

export function BrandMark({ size = 20, className }: BrandMarkProps) {
  return (
    <svg
      className={className}
      data-brand-mark="k-letter"
      viewBox="0 0 512 512"
      width={size}
      height={size}
      fill="none"
      aria-hidden="true"
      xmlns="http://www.w3.org/2000/svg"
    >
      <rect width="512" height="512" rx="104" fill="currentColor" />
      <path
        d="M154 112h72v119l104-119h88L298 247l128 153h-91L250 294l-24 27v79h-72V112Z"
        fill="var(--color-surface)"
      />
    </svg>
  );
}
