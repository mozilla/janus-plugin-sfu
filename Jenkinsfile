pipeline {
  agent any

  options {
    ansiColor('xterm')
  }

  stages {
    stage('build') {
      steps {
        build 'janus-gateway-hab-package'
      }
    }
  }

  post {
     always {
       deleteDir()
     }
   }
}
