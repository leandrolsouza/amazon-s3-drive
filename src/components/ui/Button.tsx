import { cva, type VariantProps } from "class-variance-authority";
import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef } from "react";
import { twMerge } from "tailwind-merge";

const buttonVariants = cva(
  "inline-flex cursor-pointer items-center gap-2 rounded-lg border border-solid text-left text-xs font-medium text-[var(--text-sec)] hover:text-[var(--text-main)] disabled:cursor-not-allowed disabled:opacity-50",
  {
    variants: {
      variant: {
        outline: "min-h-8 justify-center border-[var(--border-strong)] bg-[var(--bg-elevated)] px-2.5 hover:bg-[var(--bg-row-hover)]",
        navigation: "min-h-9 w-full justify-between border-transparent bg-transparent px-2.5 hover:bg-[var(--bg-row-hover)] aria-[current=page]:border-[var(--border)] aria-[current=page]:bg-[var(--bg-elevated)] aria-[current=page]:text-[var(--text-main)]",
      },
    },
    defaultVariants: { variant: "outline" },
  },
);

type ButtonProps = ComponentPropsWithoutRef<"button"> & VariantProps<typeof buttonVariants>;

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, type = "button", ...props }, ref) => (
    <button ref={ref} type={type} className={twMerge(clsx(buttonVariants({ variant }), className))} {...props} />
  ),
);
Button.displayName = "Button";
