import {
  createContext,
  forwardRef,
  type AnchorHTMLAttributes,
  type ReactNode,
  useContext,
  useMemo,
  useState,
} from "react";

export interface AppLocation {
  pathname: string;
  state?: unknown;
}

interface NavigateOptions {
  state?: unknown;
}

type Navigate = (pathname: string, options?: NavigateOptions) => void;

const RouterContext = createContext<{
  location: AppLocation;
  navigate: Navigate;
} | null>(null);

export const Router = ({
  initialLocation,
  children,
}: {
  initialLocation: AppLocation;
  children: ReactNode;
}) => {
  const [location, setLocation] = useState(initialLocation);
  const value = useMemo(
    () => ({
      location,
      navigate: (pathname: string, options?: NavigateOptions) =>
        setLocation({ pathname, state: options?.state }),
    }),
    [location]
  );

  return (
    <RouterContext.Provider value={value}>{children}</RouterContext.Provider>
  );
};

const useRouter = () => {
  const router = useContext(RouterContext);
  if (!router) {
    throw new Error("Router components must be rendered inside Router");
  }
  return router;
};

export const useLocation = () => useRouter().location;
export const useNavigate = () => useRouter().navigate;

interface LinkProps extends AnchorHTMLAttributes<HTMLAnchorElement> {
  to: string;
}

export const Link = forwardRef<HTMLAnchorElement, LinkProps>(
  ({ to, onClick, ...props }, ref) => {
    const navigate = useNavigate();
    return (
      <a
        {...props}
        ref={ref}
        href={to}
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) {
            event.preventDefault();
            navigate(to);
          }
        }}
      />
    );
  }
);

Link.displayName = "Link";
