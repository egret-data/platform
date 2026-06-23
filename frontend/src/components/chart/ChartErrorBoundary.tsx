import { Component, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** When this value changes, the boundary resets — so a fixed/re-queried chart recovers
   *  instead of staying stuck on a previous error. */
  resetKey?: unknown;
}
interface State {
  error: Error | null;
}

/** Contains a chart render error to its own card. Without this, one bad chart (e.g. an ECharts
 *  layout error like a cyclic sankey) throws during render and blanks the entire dashboard. */
export class ChartErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidUpdate(prev: Props) {
    if (prev.resetKey !== this.props.resetKey && this.state.error) {
      this.setState({ error: null });
    }
  }

  render() {
    if (this.state.error) {
      return (
        <div className="flex h-full items-center justify-center p-3 text-center text-sm text-destructive">
          Couldn't render this chart: {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}
