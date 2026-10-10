interface Env {
  EXVLAN_CORE: DurableObjectNamespace<
    import("./worker").ExvlanCoreObject
  >;
  EXVLAN_NETWORK_NAME: string;
  EXVLAN_NETWORK_SECRET: string;
  EXVLAN_ENABLE_ENCRYPTION?: string;
  EXVLAN_OBJECT_NAME?: string;
}
