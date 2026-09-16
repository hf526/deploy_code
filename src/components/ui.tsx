import React from "react";
import { AlertTriangle, CheckCircle2, Inbox, Info, Loader2, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { useApp } from "../lib/store";
import { cn } from "../lib/utils";

// ---------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "success";
type ButtonSize = "sm" | "md" | "lg";

const buttonVariants: Record<ButtonVariant, string> = {
  primary: "ui-btn-primary",
  secondary: "ui-btn-secondary",
  ghost: "ui-btn-ghost",
  danger: "ui-btn-danger",
  success: "ui-btn-success",
};

const buttonSizes: Record<ButtonSize, string> = {
  sm: "h-7 gap-1.5 rounded px-2.5 text-xs",
  md: "h-8 gap-1.5 rounded-md px-3 text-[13px]",
  lg: "h-9 gap-2 rounded-md px-4 text-[13px]",
};

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  icon?: React.ReactNode;
}

export function Button({
  variant = "primary",
  size = "md",
  loading = false,
  icon,
  className,
  children,
  disabled,
  ...rest
}: ButtonProps) {
  return (
    <button
      className={cn(
        "ui-btn inline-flex shrink-0 items-center justify-center font-medium disabled:pointer-events-none disabled:opacity-50",
        buttonVariants[variant],
        buttonSizes[size],
        className,
      )}
      disabled={disabled || loading}
      {...rest}
    >
      {loading ? <Loader2 className="size-3.5 animate-spin" /> : icon}
      {children}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Form controls
// ---------------------------------------------------------------------------

export const inputClass =
  "ui-input h-8 w-full rounded-md px-2.5 text-[13px] text-ink transition-colors placeholder:text-ink-faint disabled:opacity-50";

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, ...rest }, ref) => <input ref={ref} className={cn(inputClass, className)} {...rest} />,
);
Input.displayName = "Input";

export const Select = React.forwardRef<
  HTMLSelectElement,
  React.SelectHTMLAttributes<HTMLSelectElement>
>(({ className, children, ...rest }, ref) => (
  <select ref={ref} className={cn(inputClass, "pr-1", className)} {...rest}>
    {children}
  </select>
));
Select.displayName = "Select";

export function Field({
  label,
  hint,
  required,
  children,
  className,
}: {
  label: string;
  hint?: string;
  required?: boolean;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <label className={cn("block", className)}>
      <span className="mb-1 flex items-baseline gap-1 text-xs font-medium text-ink-dim">
        {label}
        {required && <span className="text-neg">*</span>}
        {hint && <span className="ml-auto text-[11px] font-normal text-ink-faint">{hint}</span>}
      </span>
      {children}
    </label>
  );
}

export function Checkbox({
  checked,
  onChange,
  disabled,
  children,
  className,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <label
      className={cn(
        "flex cursor-pointer items-center gap-2.5 text-xs text-ink select-none",
        disabled && "pointer-events-none opacity-50",
        className,
      )}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        className="size-3.5 accent-primary"
      />
      {children}
    </label>
  );
}

// ---------------------------------------------------------------------------
// Badge / Card / EmptyState
// ---------------------------------------------------------------------------

type BadgeKind = "gray" | "green" | "red" | "amber" | "brand";

const badgeKinds: Record<BadgeKind, string> = {
  gray: "ui-badge-gray",
  green: "ui-badge-pos",
  red: "ui-badge-neg",
  amber: "ui-badge-warn",
  brand: "ui-badge-brand",
};

export function Badge({
  kind = "gray",
  className,
  children,
}: {
  kind?: BadgeKind;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <span
      className={cn(
        "ui-badge",
        badgeKinds[kind],
        className,
      )}
    >
      {children}
    </span>
  );
}

export function Card({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div className={cn("ui-card", className)}>{children}</div>
  );
}

export function SectionTitle({
  title,
  description,
  actions,
}: {
  title: string;
  description?: string;
  actions?: React.ReactNode;
}) {
  return (
    <div className="mb-3 flex items-end justify-between gap-4">
      <div>
        <h2 className="text-[13px] font-semibold tracking-tight text-ink">{title}</h2>
        {description && <p className="mt-0.5 text-xs text-ink-dim">{description}</p>}
      </div>
      {actions}
    </div>
  );
}

export function EmptyState({
  icon,
  title,
  description,
  action,
}: {
  icon?: React.ReactNode;
  title: string;
  description?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center rounded-lg border border-dashed border-line-strong bg-panel px-6 py-14 text-center">
      <span className="mb-3 grid size-9 place-items-center rounded-md border border-line bg-field text-ink-faint">
        {icon ?? <Inbox className="size-4.5" />}
      </span>
      <p className="text-[13px] font-medium text-ink">{title}</p>
      {description && <p className="mt-1 max-w-md text-xs leading-relaxed text-ink-dim">{description}</p>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Page layout
// ---------------------------------------------------------------------------

export function Page({
  title,
  subtitle,
  actions,
  children,
  scroll = true,
}: {
  title: string;
  subtitle?: string;
  actions?: React.ReactNode;
  children: React.ReactNode;
  scroll?: boolean;
}) {
  return (
    <div className="flex h-full flex-col">
      <header className="ui-titlebar flex shrink-0 items-center justify-between gap-6 border-b border-line px-6 py-3.5">
        <div className="min-w-0">
          <h1 className="truncate text-[17px] font-semibold tracking-tight text-ink">{title}</h1>
          {subtitle && <p className="mt-0.5 truncate text-[12.5px] text-ink-dim">{subtitle}</p>}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </header>
      <div className={cn("min-h-0 flex-1 px-6 py-5", scroll && "overflow-y-auto")}>{children}</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Modal / Confirm
// ---------------------------------------------------------------------------

export function Modal({
  open,
  onClose,
  title,
  subtitle,
  footer,
  width = "max-w-lg",
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  subtitle?: string;
  footer?: React.ReactNode;
  width?: string;
  children: React.ReactNode;
}) {
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-6">
      <div className="absolute inset-0 bg-black/40" onClick={onClose} />
      <div
        className={cn(
          "ui-pop relative z-10 flex max-h-[86vh] w-full flex-col overflow-hidden",
          width,
        )}
      >
        <div className="flex shrink-0 items-start justify-between gap-4 border-b border-line px-5 py-3.5">
          <div>
            <h3 className="text-sm font-semibold text-ink">{title}</h3>
            {subtitle && <p className="mt-0.5 text-xs text-ink-dim">{subtitle}</p>}
          </div>
          <button
            onClick={onClose}
            className="rounded p-1 text-ink-faint transition-colors hover:bg-hover hover:text-ink"
          >
            <X className="size-4" />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">{children}</div>
        {footer && (
          <div className="flex shrink-0 justify-end gap-2 border-t border-line px-5 py-3.5">
            {footer}
          </div>
        )}
      </div>
    </div>
  );
}

export function ConfirmModal({
  open,
  title,
  description,
  confirmText,
  danger = false,
  loading = false,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  title: string;
  description: React.ReactNode;
  confirmText?: string;
  danger?: boolean;
  loading?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Modal
      open={open}
      onClose={loading ? () => undefined : onCancel}
      title={title}
      width="max-w-md"
      footer={
        <>
          <Button variant="secondary" disabled={loading} onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button variant={danger ? "danger" : "primary"} loading={loading} onClick={onConfirm}>
            {confirmText ?? t("common.confirm")}
          </Button>
        </>
      }
    >
      <div className="flex gap-3">
        <span className={cn("mt-0.5 shrink-0", danger ? "text-neg" : "text-warn")}>
          <AlertTriangle className="size-4.5" />
        </span>
        <div className="text-[13px] leading-relaxed text-ink-dim">{description}</div>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------

export function Toasts() {
  const toasts = useApp((state) => state.toasts);
  const dismiss = useApp((state) => state.dismissToast);

  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-[100] flex w-88 flex-col gap-2">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={cn(
            "ui-pop pointer-events-auto flex items-start gap-2.5 px-3.5 py-2.5",
            toast.kind === "success" && "border-pos/35",
            toast.kind === "error" && "border-neg/35",
          )}
        >
          <span className="mt-0.5 shrink-0">
            {toast.kind === "success" ? (
              <CheckCircle2 className="size-4 text-pos" />
            ) : toast.kind === "error" ? (
              <AlertTriangle className="size-4 text-neg" />
            ) : (
              <Info className="size-4 text-brand" />
            )}
          </span>
          <p className="min-w-0 flex-1 whitespace-pre-wrap break-words text-xs leading-relaxed text-ink">
            {toast.message}
          </p>
          <button
            onClick={() => dismiss(toast.id)}
            className="shrink-0 rounded p-0.5 text-ink-faint transition-colors hover:text-ink"
          >
            <X className="size-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}
