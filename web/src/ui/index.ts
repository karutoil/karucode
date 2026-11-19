export { cn, variants, type ClassValue, type CompoundVariant, type VariantProps, type VariantsConfig } from './variants';
export type {
  BaseUiProps,
  FeedbackTone,
  SpaceStep,
  StatusDotTone,
  SurfaceTone,
  TextSize,
  TextTone,
} from './types';

export {
  Button,
  IconButton,
  LinkButton,
  buttonVariants,
  iconButtonVariants,
  linkButtonVariants,
  type ButtonProps,
  type ButtonSize,
  type ButtonVariant,
  type IconButtonProps,
  type LinkButtonProps,
  type LinkButtonTone,
} from './Button';

export { ConfirmDialog, type ConfirmDialogProps, type ConfirmDialogTone } from './ConfirmDialog';

export { Field, fieldVariants, type FieldA11yProps, type FieldProps } from './Field';
export {
  Checkbox,
  Input,
  Select,
  Textarea,
  inputVariants,
  selectVariants,
  textareaVariants,
  type CheckboxProps,
  type InputProps,
  type SelectOption,
  type SelectProps,
  type TextareaProps,
} from './Input';

export {
  Card,
  Inline,
  SectionHeader,
  SettingRow,
  Stack,
  Text,
  type CardProps,
  type InlineProps,
  type SectionHeaderProps,
  type SettingRowProps,
  type StackProps,
  type TextProps,
} from './Layout';

export {
  Badge,
  Callout,
  Chip,
  Keycap,
  Skeleton,
  Spinner,
  StatusDot,
  type BadgeProps,
  type CalloutProps,
  type CalloutTone,
  type ChipProps,
  type KeycapProps,
  type SkeletonProps,
  type SpinnerProps,
  type StatusDotProps,
} from './Feedback';

// Graduated shell & accessibility primitives from `src/components/`
export { Overlay, type OverlayBodyLayout, type OverlayProps, type OverlayRole } from '../components/Overlay';
export { Dialog, type DialogProps } from '../components/Dialog';
export { TabPanel, Tabs, type TabItem, type TabPanelProps, type TabVariant, type TabsProps } from '../components/Tabs';
export { Toggle, type ToggleProps } from '../components/Toggle';
export { EmptyState, QUICK_PROMPTS, type EmptyStateProps, type QuickPrompt } from '../components/EmptyState';
export { ErrorBoundary, diagnostics, type ErrorBoundaryProps } from '../components/ErrorBoundary';
export * as icons from '../components/icons';
