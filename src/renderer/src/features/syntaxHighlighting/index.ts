// Shared Renderer syntax-highlighting boundary backed by the existing Shiki worker service.
export {
  useGitReviewSyntaxHighlight as useSyntaxHighlight,
  type GitReviewSyntaxHighlightHookInput as SyntaxHighlightHookInput,
  type GitReviewSyntaxHighlightState as SyntaxHighlightState
} from '../gitReview/syntaxHighlighting'
export type {
  GitReviewHighlightLine as SyntaxHighlightLine,
  GitReviewHighlightResult as SyntaxHighlightResult,
  GitReviewHighlightToken as SyntaxHighlightToken
} from '../gitReview/syntaxHighlighting'
