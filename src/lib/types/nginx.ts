export interface NginxContainerInfo {
  name: string;
  image: string;
  ports: string;
  /** 名称或镜像包含 nginx，界面上优先展示。 */
  nginx: boolean;
}

export interface NginxConfigFile {
  name: string;
  size: number;
}

export interface NginxConfigContent {
  name: string;
  path: string;
  content: string;
}
